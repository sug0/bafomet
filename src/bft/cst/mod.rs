//! The collaborative state transfer algorithm.
//!
//! The implementation is based on the paper «On the Efﬁciency of
//! Durable State Machine Replication», by A. Bessani et al.

// NOTE: in this module, we may use cid interchangeably with
// consensus sequence number

use std::cmp::Ordering;
use std::time::Duration;

#[cfg(feature = "serialize_serde")]
use serde::{Deserialize, Serialize};

use crate::bft::collections::{self, HashMap};
use crate::bft::communication::message::{CstMessage, CstMessageKind, Header, SystemMessage};
use crate::bft::communication::{Node, NodeId};
use crate::bft::consensus::log::{Checkpoint, DecisionLog, Log};
use crate::bft::consensus::Consensus;
use crate::bft::core::server::ViewInfo;
use crate::bft::crypto::hash::Digest;
use crate::bft::error::*;
use crate::bft::executable::{ExecutorHandle, Reply, Request, Service, State};
use crate::bft::ordering::{Orderable, SeqNo};
use crate::bft::sync::Synchronizer;
use crate::bft::timeouts::{TimeoutKind, TimeoutsHandle};

enum ProtoPhase<S, O> {
    Init,
    WaitingCheckpoint(Header, CstMessage<S, O>),
    ReceivingCid(usize),
    ReceivingState(usize),
}

/// Contains state used by a recovering node.
#[cfg_attr(feature = "serialize_serde", derive(Serialize, Deserialize))]
#[derive(Clone)]
pub struct RecoveryState<S, O> {
    pub(crate) view: ViewInfo,
    pub(crate) checkpoint: Checkpoint<S>,
    // used to replay log on recovering replicas;
    // the request batches have been concatenated,
    // for memory efficiency
    pub(crate) requests: Vec<O>,
    pub(crate) declog: DecisionLog,
}

/// Allow a replica to recover from the state received by peer nodes.
pub fn install_recovery_state<S>(
    recovery_state: RecoveryState<State<S>, Request<S>>,
    synchronizer: &mut Synchronizer<S>,
    log: &mut Log<State<S>, Request<S>, Reply<S>>,
    executor: &mut ExecutorHandle<S>,
    consensus: &mut Consensus<S>,
) -> Result<()>
where
    S: Service + Send + 'static,
    State<S>: Send + Clone + 'static,
    Request<S>: Send + Clone + 'static,
    Reply<S>: Send + 'static,
{
    // TODO: maybe try to optimize this, to avoid clone(),
    // which may be quite expensive depending on the size
    // of the state and the amount of batched requests
    let state = recovery_state.checkpoint.state().clone();
    let requests = recovery_state.requests.clone();

    // TODO: update pub/priv keys when reconfig is implemented?

    synchronizer.install_view(recovery_state.view);
    consensus.install_new_phase(&recovery_state);
    executor.install_state(state, requests)?;
    log.install_state(consensus.sequence_number(), recovery_state);

    Ok(())
}

impl<S, O> RecoveryState<S, O> {
    /// Creates a new `RecoveryState`.
    pub fn new(
        view: ViewInfo,
        checkpoint: Checkpoint<S>,
        requests: Vec<O>,
        declog: DecisionLog,
    ) -> Self {
        Self {
            view,
            checkpoint,
            requests,
            declog,
        }
    }

    /// Returns the view this `RecoveryState` is tracking.
    pub fn view(&self) -> ViewInfo {
        self.view
    }

    /// Returns the local checkpoint of this recovery state.
    pub fn checkpoint(&self) -> &Checkpoint<S> {
        &self.checkpoint
    }

    /// Returns the operations embedded in the requests sent by clients
    /// after the last checkpoint at the moment of the creation of this `RecoveryState`.
    pub fn requests(&self) -> &[O] {
        &self.requests[..]
    }

    /// Returns a reference to the decided consensus messages of this recovery state.
    pub fn decision_log(&self) -> &DecisionLog {
        &self.declog
    }
}

struct ReceivedState<S, O> {
    count: usize,
    state: RecoveryState<S, O>,
}

/// Represents the state of an on-going colloborative
/// state transfer protocol execution.
pub struct CollabStateTransfer<S: Service> {
    latest_cid: SeqNo,
    cst_seq: SeqNo,
    latest_cid_count: usize,
    base_timeout: Duration,
    curr_timeout: Duration,
    // NOTE: remembers whose replies we have
    // received already, to avoid replays
    //voted: HashSet<NodeId>,
    received_states: HashMap<Digest, ReceivedState<State<S>, Request<S>>>,
    phase: ProtoPhase<State<S>, Request<S>>,
}

/// Status returned from processnig a state transfer message.
pub enum CstStatus<S, O> {
    /// We are not running the CST protocol.
    ///
    /// Drop any attempt of processing a message in this condition.
    Nil,
    /// The CST protocol is currently running.
    Running,
    /// We should request the latest cid from the view.
    RequestLatestCid,
    /// We should request the latest state from the view.
    RequestState,
    /// We have received and validated the largest consensus sequence
    /// number available.
    SeqNo(SeqNo),
    /// We have received and validated the state from
    /// a group of replicas.
    State(RecoveryState<S, O>),
}

/// Represents progress in the CST state machine.
///
/// To clarify, the mention of state machine here has nothing to do with the
/// SMR protocol, but rather the implementation in code of the CST protocol.
pub enum CstProgress<S, O> {
    // TODO: Timeout( some type here)
    /// This value represents null progress in the CST code's state machine.
    Nil,
    /// We have a fresh new message to feed the CST state machine, from
    /// the communication layer.
    Message(Header, CstMessage<S, O>),
}

macro_rules! getmessage {
    ($progress:expr, $status:expr) => {
        match $progress {
            CstProgress::Nil => return $status,
            CstProgress::Message(h, m) => (h, m),
        }
    };
    // message queued while waiting for exec layer to deliver app state
    ($phase:expr) => {{
        let phase = std::mem::replace($phase, ProtoPhase::Init);
        match phase {
            ProtoPhase::WaitingCheckpoint(h, m) => (h, m),
            _ => return CstStatus::Nil,
        }
    }};
}

// TODO: request timeouts
impl<S> CollabStateTransfer<S>
where
    S: Service + Send + 'static,
    State<S>: Send + Clone + 'static,
    Request<S>: Send + Clone + 'static,
    Reply<S>: Send + 'static,
{
    /// Craete a new instance of `CollabStateTransfer`.
    pub fn new(base_timeout: Duration) -> Self {
        Self {
            base_timeout,
            curr_timeout: base_timeout,
            received_states: collections::hash_map(),
            phase: ProtoPhase::Init,
            latest_cid: SeqNo::ZERO,
            latest_cid_count: 0,
            cst_seq: SeqNo::ZERO,
        }
    }

    /// Checks if the CST layer is waiting for a local checkpoint to
    /// complete.
    ///
    /// This is used when a node is sending state to a peer.
    pub fn needs_checkpoint(&self) -> bool {
        matches!(self.phase, ProtoPhase::WaitingCheckpoint(_, _))
    }

    fn process_reply_state(
        &mut self,
        header: Header,
        message: CstMessage<State<S>, Request<S>>,
        synchronizer: &Synchronizer<S>,
        log: &Log<State<S>, Request<S>, Reply<S>>,
        node: &mut Node<S::Data>,
    ) {
        let snapshot = match log.snapshot(*synchronizer.view()) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                self.phase = ProtoPhase::WaitingCheckpoint(header, message);
                return;
            }
        };
        let reply = SystemMessage::Cst(CstMessage::new(
            message.sequence_number(),
            CstMessageKind::ReplyState(snapshot),
        ));
        node.send(reply, header.from());
    }

    /// Advances the state of the CST state machine.
    pub fn process_message(
        &mut self,
        progress: CstProgress<State<S>, Request<S>>,
        synchronizer: &Synchronizer<S>,
        consensus: &Consensus<S>,
        log: &Log<State<S>, Request<S>, Reply<S>>,
        node: &mut Node<S::Data>,
    ) -> CstStatus<State<S>, Request<S>> {
        match self.phase {
            ProtoPhase::WaitingCheckpoint(_, _) => {
                let (header, message) = getmessage!(&mut self.phase);
                self.process_reply_state(header, message, synchronizer, log, node);
                CstStatus::Nil
            }
            ProtoPhase::Init => {
                let (header, message) = getmessage!(progress, CstStatus::Nil);
                match message.kind() {
                    CstMessageKind::RequestLatestConsensusSeq => {
                        let kind =
                            CstMessageKind::ReplyLatestConsensusSeq(consensus.sequence_number());
                        let reply =
                            SystemMessage::Cst(CstMessage::new(message.sequence_number(), kind));
                        node.send(reply, header.from());
                    }
                    CstMessageKind::RequestState => {
                        self.process_reply_state(header, message, synchronizer, log, node);
                    }
                    // we are not running cst, so drop any reply msgs
                    //
                    // TODO: maybe inspect cid msgs, and passively start
                    // the state transfer protocol, by returning
                    // CstStatus::RequestState
                    _ => (),
                }
                CstStatus::Nil
            }
            ProtoPhase::ReceivingCid(i) => {
                let (_header, message) = getmessage!(progress, CstStatus::RequestLatestCid);

                // drop cst messages with invalid seq no
                if message.sequence_number() != self.cst_seq {
                    // FIXME: how to handle old or newer messages?
                    // BFT-SMaRt simply ignores messages with a
                    // value of `queryID` different from the current
                    // `queryID` a replica is tracking...
                    // we will do the same for now
                    //
                    // TODO: implement timeouts to fix cases like this
                    return CstStatus::Running;
                }

                match message.kind() {
                    CstMessageKind::ReplyLatestConsensusSeq(seq) => {
                        match seq.cmp(&self.latest_cid) {
                            Ordering::Greater => {
                                self.latest_cid = *seq;
                                self.latest_cid_count = 1;
                            }
                            Ordering::Equal => {
                                self.latest_cid_count += 1;
                            }
                            Ordering::Less => (),
                        }
                    }
                    // drop invalid message kinds
                    _ => return CstStatus::Running,
                }

                // check if we have gathered enough cid
                // replies from peer nodes
                //
                // TODO: check for more than one reply from the same node
                let i = i + 1;

                if i == synchronizer.view().params().quorum() {
                    self.phase = ProtoPhase::Init;
                    if self.latest_cid_count > synchronizer.view().params().f() {
                        // reset timeout, since req was successful
                        self.curr_timeout = self.base_timeout;

                        // the latest cid was available in at least
                        // f+1 replicas
                        CstStatus::SeqNo(self.latest_cid)
                    } else {
                        CstStatus::RequestLatestCid
                    }
                } else {
                    self.phase = ProtoPhase::ReceivingCid(i);
                    CstStatus::Running
                }
            }
            ProtoPhase::ReceivingState(i) => {
                let (header, mut message) = getmessage!(progress, CstStatus::RequestState);

                // NOTE: check comment above, on ProtoPhase::ReceivingCid
                if message.sequence_number() != self.cst_seq {
                    return CstStatus::Running;
                }

                let state = match message.take_state() {
                    Some(state) => state,
                    // drop invalid message kinds
                    None => return CstStatus::Running,
                };

                let received_state = self
                    .received_states
                    .entry(header.digest().clone())
                    .or_insert(ReceivedState { count: 0, state });

                received_state.count += 1;

                // check if we have gathered enough state
                // replies from peer nodes
                //
                // TODO: check for more than one reply from the same node
                let i = i + 1;

                if i != synchronizer.view().params().quorum() {
                    self.phase = ProtoPhase::ReceivingState(i);
                    return CstStatus::Running;
                }

                // NOTE: clear saved states when we return;
                // this is important, because each state
                // may be several GBs in size

                // check if we have at least f+1 matching states
                let digest = {
                    let received_state = self.received_states.iter().max_by_key(|(_, st)| st.count);
                    match received_state {
                        Some((digest, _)) => digest.clone(),
                        None => {
                            self.received_states.clear();
                            return CstStatus::RequestState;
                        }
                    }
                };
                let received_state = {
                    let received_state = self.received_states.remove(&digest);
                    self.received_states.clear();
                    received_state
                };

                // reset timeout, since req was successful
                self.curr_timeout = self.base_timeout;

                // return the state
                let f = synchronizer.view().params().f();
                match received_state {
                    Some(ReceivedState { count, state }) if count > f => CstStatus::State(state),
                    _ => CstStatus::RequestState,
                }
            }
        }
    }

    fn next_seq(&mut self) -> SeqNo {
        let next = self.cst_seq;
        self.cst_seq = self.cst_seq.next();
        next
    }

    /// Handle a timeout received from the timeouts layer.
    pub fn timed_out(&mut self, seq: SeqNo) -> CstStatus<State<S>, Request<S>> {
        if seq.next() != self.cst_seq {
            // the timeout we received is for a request
            // that has already completed, therefore we ignore it
            //
            // TODO: this check is probably not necessary,
            // as we have likely already updated the `ProtoPhase`
            // to reflect the fact we are no longer receiving state
            // from peer nodes
            return CstStatus::Nil;
        }
        match self.phase {
            // retry requests if receiving state and we have timed out
            ProtoPhase::ReceivingCid(_) => {
                self.curr_timeout *= 2;
                CstStatus::RequestLatestCid
            }
            ProtoPhase::ReceivingState(_) => {
                self.curr_timeout *= 2;
                CstStatus::RequestState
            }
            // ignore timeouts if not receiving any kind
            // of state from peer nodes
            _ => CstStatus::Nil,
        }
    }

    /// Used by a recovering node to retrieve the latest sequence number
    /// attributed to a client request by the consensus layer.
    pub fn request_latest_consensus_seq_no(
        &mut self,
        synchronizer: &Synchronizer<S>,
        timeouts: &TimeoutsHandle<S>,
        node: &mut Node<S::Data>,
    ) {
        // reset state of latest seq no. request
        self.latest_cid = SeqNo::ZERO;
        self.latest_cid_count = 0;

        let cst_seq = self.next_seq();
        timeouts.timeout(self.curr_timeout, TimeoutKind::Cst(cst_seq));
        self.phase = ProtoPhase::ReceivingCid(0);

        let message = SystemMessage::Cst(CstMessage::new(
            cst_seq,
            CstMessageKind::RequestLatestConsensusSeq,
        ));
        let targets = NodeId::targets(0..synchronizer.view().params().n());
        node.broadcast(message, targets);
    }

    /// Used by a recovering node to retrieve the latest state.
    pub fn request_latest_state(
        &mut self,
        synchronizer: &Synchronizer<S>,
        timeouts: &TimeoutsHandle<S>,
        node: &mut Node<S::Data>,
    ) {
        // reset hashmap of received states
        self.received_states.clear();

        let cst_seq = self.next_seq();
        timeouts.timeout(self.curr_timeout, TimeoutKind::Cst(cst_seq));
        self.phase = ProtoPhase::ReceivingState(0);

        let message = SystemMessage::Cst(CstMessage::new(cst_seq, CstMessageKind::RequestState));
        let targets = NodeId::targets(0..synchronizer.view().params().n());
        node.broadcast(message, targets);
    }
}
