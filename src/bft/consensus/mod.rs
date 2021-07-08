//! The consensus algorithm used for `febft` and other logic.

use std::marker::PhantomData;
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};

use either::{
    Left,
    Right,
};

use crate::bft::log::Log;
use crate::bft::ordering::SeqNo;
use crate::bft::cst::RecoveryState;
use crate::bft::crypto::hash::Digest;
use crate::bft::core::server::ViewInfo;
use crate::bft::communication::message::{
    Header,
    SystemMessage,
    ConsensusMessage,
    ConsensusMessageKind,
};
//use crate::bft::collections::{
//    self,
//    HashSet,
//};
use crate::bft::communication::{
    Node,
    NodeId,
};
use crate::bft::executable::{
    Service,
    Request,
    Reply,
    State,
};

/// Represents the status of calling `poll()` on a `TboQueue`.
pub enum ConsensusPollStatus {
    /// The `Replica` associated with this `TboQueue` should
    /// poll its main channel for more messages.
    Recv,
    /// The `Replica` associated with this `TboQueue` should
    /// propose a new client request to be ordered, if it is
    /// the leader, and then it should poll its main channel
    /// for more messages. Alternatively, if the request has
    /// already been decided, it should be queued for
    /// execution.
    TryProposeAndRecv,
    /// A new consensus message is available to be processed.
    NextMessage(Header, ConsensusMessage),
}

/// Represents a queue of messages to be ordered in a consensus instance.
///
/// Because of the asynchrony of the Internet, messages may arrive out of
/// context, e.g. for the same consensus instance, a `PRE-PREPARE` reaches
/// a node after a `PREPARE`. A `TboQueue` arranges these messages to be
/// processed in the correct order.
pub struct TboQueue {
    curr_seq: SeqNo,
    get_queue: bool,
    pre_prepares: VecDeque<VecDeque<(Header, ConsensusMessage)>>,
    prepares: VecDeque<VecDeque<(Header, ConsensusMessage)>>,
    commits: VecDeque<VecDeque<(Header, ConsensusMessage)>>,
}

// XXX: details
impl TboQueue {
    fn new_impl(curr_seq: SeqNo) -> Self {
        Self {
            curr_seq,
            get_queue: false,
            pre_prepares: VecDeque::new(),
            prepares: VecDeque::new(),
            commits: VecDeque::new(),
        }
    }

    fn pop_message(
        tbo: &mut VecDeque<VecDeque<(Header, ConsensusMessage)>>,
    ) -> Option<(Header, ConsensusMessage)> {
        if tbo.is_empty() {
            None
        } else {
            tbo[0].pop_front()
        }
    }

    fn queue_message(
        curr_seq: SeqNo,
        tbo: &mut VecDeque<VecDeque<(Header, ConsensusMessage)>>,
        h: Header,
        m: ConsensusMessage,
    ) {
        let index = match m.sequence_number().index(curr_seq) {
            Right(i) => i,
            Left(_) => {
                // FIXME: maybe notify peers if we detect a message
                // with an invalid (too large) seq no? return the
                // `NodeId` of the offending node.
                return;
            },
        };
        if index >= tbo.len() {
            let len = index - tbo.len() + 1;
            tbo.extend(std::iter::repeat_with(VecDeque::new).take(len));
        }
        tbo[index].push_back((h, m));
    }

    fn advance_message_queue(
        tbo: &mut VecDeque<VecDeque<(Header, ConsensusMessage)>>,
    ) {
        match tbo.pop_front() {
            Some(mut vec) => {
                // recycle memory
                vec.clear();
                tbo.push_back(vec);
            },
            None => (),
        }
    }
}

// XXX: api
impl TboQueue {
    fn new(curr_seq: SeqNo) -> Self {
        Self::new_impl(curr_seq)
    }

    /// Signal this `TboQueue` that it may be able to extract new
    /// consensus messages from its internal storage.
    pub fn signal(&mut self) {
        self.get_queue = true;
    }

    /// Reports the id of the consensus this `TboQueue` is tracking.
    pub fn sequence_number(&self) -> SeqNo {
        self.curr_seq
    }

    /// Advances the message queue, and updates the consensus instance id.
    fn next_instance_queue(&mut self) {
        self.curr_seq = self.curr_seq.next();
        Self::advance_message_queue(&mut self.pre_prepares);
        Self::advance_message_queue(&mut self.prepares);
        Self::advance_message_queue(&mut self.commits);
    }

    /// Queues a consensus message for later processing, or drops it
    /// immediately if it pertains to an older consensus instance.
    pub fn queue(&mut self, h: Header, m: ConsensusMessage) {
        match m.kind() {
            ConsensusMessageKind::PrePrepare(_) => self.queue_pre_prepare(h, m),
            ConsensusMessageKind::Prepare => self.queue_prepare(h, m),
            ConsensusMessageKind::Commit => self.queue_commit(h, m),
        }
    }

    /// Queues a `PRE-PREPARE` message for later processing, or drops it
    /// immediately if it pertains to an older consensus instance.
    fn queue_pre_prepare(&mut self, h: Header, m: ConsensusMessage) {
        Self::queue_message(self.curr_seq, &mut self.pre_prepares, h, m)
    }

    /// Queues a `PREPARE` message for later processing, or drops it
    /// immediately if it pertains to an older consensus instance.
    fn queue_prepare(&mut self, h: Header, m: ConsensusMessage) {
        Self::queue_message(self.curr_seq, &mut self.prepares, h, m)
    }

    /// Queues a `COMMIT` message for later processing, or drops it
    /// immediately if it pertains to an older consensus instance.
    fn queue_commit(&mut self, h: Header, m: ConsensusMessage) {
        Self::queue_message(self.curr_seq, &mut self.commits, h, m)
    }
}

/// Repreents the current phase of the consensus protocol.
#[derive(Debug, Copy, Clone)]
pub enum ProtoPhase {
    /// Start of a new consensus instance.
    Init,
    /// Running the `PRE-PREPARE` phase.
    PrePreparing,
    /// A replica has accepted a `PRE-PREPARE` message, but
    /// it doesn't have the entirety of the requests it references
    /// in its log.
    PreparingRequests,
    /// Running the `PREPARE` phase. The integer represents
    /// the number of votes received.
    Preparing(usize),
    /// Running the `COMMIT` phase. The integer represents
    /// the number of votes received.
    Committing(usize),
}

/// Contains the state of an active consensus instance, as well
/// as future instances.
pub struct Consensus<S: Service> {
    // can be smaller than the config's max batch size,
    // but never longer
    batch_size: usize,
    phase: ProtoPhase,
    tbo: TboQueue,
    current: Vec<Digest>,
    //voted: HashSet<NodeId>,
    missing_requests: VecDeque<Digest>,
    missing_swapbuf: Vec<usize>,
    _phantom: PhantomData<S>,
}

/// Status returned from processing a consensus message.
pub enum ConsensusStatus<'a> {
    /// A particular node tried voting twice.
    VotedTwice(NodeId),
    /// A `febft` quorum still hasn't made a decision
    /// on a client request to be executed.
    Deciding,
    /// A `febft` quorum decided on the execution of
    /// the batch of requests with the given digests.
    Decided(&'a [Digest]),
}

macro_rules! extract_msg {
    ($g:expr, $q:expr) => {
        extract_msg!({}, $g, $q)
    };

    ($opt:block, $g:expr, $q:expr) => {
        if let Some((header, message)) = TboQueue::pop_message($q) {
            $opt
            ConsensusPollStatus::NextMessage(header, message)
        } else {
            *$g = false;
            ConsensusPollStatus::Recv
        }
    };
}

impl<S> Consensus<S>
where
    S: Service + Send + 'static,
    State<S>: Send + Clone + 'static,
    Request<S>: Send + 'static,
    Reply<S>: Send + 'static,
{
    /// Starts a new consensus protocol tracker.
    pub fn new(initial_seq_no: SeqNo, batch_size: usize) -> Self {
        Self {
            batch_size: 0,
            _phantom: PhantomData,
            phase: ProtoPhase::Init,
            missing_swapbuf: Vec::new(),
            missing_requests: VecDeque::new(),
            //voted: collections::hash_set(),
            tbo: TboQueue::new(initial_seq_no),
            current: std::iter::repeat_with(|| Digest::from_bytes(&[0; Digest::LENGTH][..]))
                .flat_map(|d| d) // unwrap
                .take(batch_size)
                .collect(),
        }
    }

    /// Update the consensus protocol phase, according to the state
    /// received from peer nodes in the CST protocol.
    pub fn install_new_phase(
        &mut self,
        view: ViewInfo,
        recovery_state: &RecoveryState<State<S>, Request<S>>,
    ) {
        // get the latest seq no
        let seq_no = {
            let pre_prepares = recovery_state.pre_prepares();
            if pre_prepares.is_empty() {
                self.sequence_number()
            } else {
                pre_prepares[pre_prepares.len() - 1]
                    .message()
                    .sequence_number()
            }
        };

        // skip old messages
        while self.sequence_number() < seq_no {
            self.next_instance();
        }

        // FIXME: update phase
        self.phase = ProtoPhase::Init;
    }

    /// Proposes a new request with digest `dig`.
    ///
    /// This function will only succeed if the `node` is
    /// the leader of the current `view` and the `node` is
    /// in the phase `ProtoPhase::Init`.
    pub fn propose(&mut self, digests: Vec<Digest>, view: ViewInfo, node: &mut Node<S::Data>) {
        match self.phase {
            ProtoPhase::Init => self.phase = ProtoPhase::PrePreparing,
            _ => return,
        }
        if node.id() != view.leader() {
            return;
        }
        let message = SystemMessage::Consensus(ConsensusMessage::new(
            self.sequence_number(),
            ConsensusMessageKind::PrePrepare(digests),
        ));
        let targets = NodeId::targets(0..view.params().n());
        node.broadcast(message, targets);
    }

    /// Returns the current protocol phase.
    pub fn phase(&self) -> ProtoPhase {
        self.phase
    }

    /// Check if we can process new consensus messages.
    pub fn poll(&mut self, log: &Log<State<S>, Request<S>, Reply<S>>) -> ConsensusPollStatus {
        match self.phase {
            ProtoPhase::Init if self.tbo.get_queue => {
                extract_msg!(
                    { self.phase = ProtoPhase::PrePreparing; },
                    &mut self.tbo.get_queue,
                    &mut self.tbo.pre_prepares
                )
            },
            ProtoPhase::Init => {
                ConsensusPollStatus::TryProposeAndRecv
            },
            ProtoPhase::PrePreparing if self.tbo.get_queue => {
                extract_msg!(&mut self.tbo.get_queue, &mut self.tbo.pre_prepares)
            },
            ProtoPhase::PreparingRequests => {
                let iterator = self.missing_requests
                    .iter()
                    .enumerate()
                    .filter(|(_index, digest)| log.has_request(digest));
                for (index, _) in iterator {
                    self.missing_swapbuf.push(index);
                }
                for index in self.missing_swapbuf.drain(..) {
                    self.missing_requests.swap_remove_back(index);
                }
                if self.missing_requests.is_empty() {
                    extract_msg!(
                        { self.phase = ProtoPhase::Preparing(0); },
                        &mut self.tbo.get_queue,
                        &mut self.tbo.prepares
                    )
                } else {
                    ConsensusPollStatus::Recv
                }
            },
            ProtoPhase::Preparing(_) if self.tbo.get_queue => {
                extract_msg!(&mut self.tbo.get_queue, &mut self.tbo.prepares)
            },
            ProtoPhase::Committing(_) if self.tbo.get_queue => {
                extract_msg!(&mut self.tbo.get_queue, &mut self.tbo.commits)
            },
            _ => ConsensusPollStatus::Recv,
        }
    }

    /// Starts a new consensus instance.
    pub fn next_instance(&mut self) {
        self.tbo.next_instance_queue();
        //self.voted.clear();
    }

    /// Sets the id of the current consensus.
    pub fn install_sequence_number(&mut self, seq: SeqNo) {
        // drop old msgs
        match seq.index(self.sequence_number()) {
            // nothing to do if we are on the same seq
            Right(0) => return,
            // drop messages up to `limit`
            Right(limit) if limit >= self.tbo.pre_prepares.len() => {
                // NOTE: optimization to avoid draining the `VecDeque`
                // structures when the difference between the seq
                // numbers is huge
                self.tbo.pre_prepares.clear();
                self.tbo.prepares.clear();
                self.tbo.commits.clear();
            },
            Right(limit) => {
                let iterator = self.tbo.pre_prepares
                    .drain(..limit)
                    .chain(self.tbo.prepares
                        .drain(..limit)
                        .chain(self.tbo.commits
                            .drain(..limit)));
                for _ in iterator {
                    // consume elems
                }
            },
            // drop all messages
            Left(_) => {
                // NOTE: same as NOTE on the match branch above
                self.tbo.pre_prepares.clear();
                self.tbo.prepares.clear();
                self.tbo.commits.clear();
            },
        }

        // install new phase
        self.tbo.curr_seq = seq;
        self.phase = ProtoPhase::Init;

        // FIXME: do we need to clear the missing requests buffers?
    }

    /// Process a message for a particular consensus instance.
    pub fn process_message<'a>(
        &'a mut self,
        header: Header,
        message: ConsensusMessage,
        view: ViewInfo,
        log: &mut Log<State<S>, Request<S>, Reply<S>>,
        node: &mut Node<S::Data>,
    ) -> ConsensusStatus<'a> {
        // FIXME: use order imposed by leader
        // FIXME: check if the pre-prepare is from the leader
        // FIXME: make sure a replica doesn't vote twice
        // by keeping track of who voted, and not just
        // the amount of votes received
        match self.phase {
            ProtoPhase::Init => {
                // in the init phase, we can't do anything,
                // queue the message for later
                match message.kind() {
                    ConsensusMessageKind::PrePrepare(_) => {
                        self.queue_pre_prepare(header, message);
                        return ConsensusStatus::Deciding;
                    },
                    ConsensusMessageKind::Prepare => {
                        self.queue_prepare(header, message);
                        return ConsensusStatus::Deciding;
                    },
                    ConsensusMessageKind::Commit => {
                        self.queue_commit(header, message);
                        return ConsensusStatus::Deciding;
                    },
                }
            },
            ProtoPhase::PrePreparing => {
                // queue message if we're not pre-preparing
                // or in the same seq as the message
                match message.kind() {
                    ConsensusMessageKind::PrePrepare(_) if message.sequence_number() != self.sequence_number() => {
                        self.queue_pre_prepare(header, message);
                        return ConsensusStatus::Deciding;
                    },
                    ConsensusMessageKind::PrePrepare(digests) => {
                        self.batch_size = digests.len();
                        (&mut self.current[..digests.len()]).copy_from_slice(&digests[..]);
                    },
                    ConsensusMessageKind::Prepare => {
                        self.queue_prepare(header, message);
                        return ConsensusStatus::Deciding;
                    },
                    ConsensusMessageKind::Commit => {
                        self.queue_commit(header, message);
                        return ConsensusStatus::Deciding;
                    },
                }
                // leader can't vote for a prepare
                if node.id() != view.leader() {
                    let message = SystemMessage::Consensus(ConsensusMessage::new(
                        self.sequence_number(),
                        ConsensusMessageKind::Prepare,
                    ));
                    let targets = NodeId::targets(0..view.params().n());
                    node.broadcast(message, targets);
                }
                // add message to the log
                log.insert(header, SystemMessage::Consensus(message));
                // try entering preparing phase
                for digest in self.current.iter().filter(|d| !log.has_request(d)) {
                    self.missing_requests.push_back(digest.clone());
                }
                self.phase = if self.missing_requests.is_empty() {
                    ProtoPhase::Preparing(0)
                } else {
                    ProtoPhase::PreparingRequests
                };
                ConsensusStatus::Deciding
            },
            ProtoPhase::PreparingRequests => {
                // can't do anything while waiting for client requests,
                // queue the message for later
                match message.kind() {
                    ConsensusMessageKind::PrePrepare(_) => {
                        self.queue_pre_prepare(header, message);
                        return ConsensusStatus::Deciding;
                    },
                    ConsensusMessageKind::Prepare => {
                        self.queue_prepare(header, message);
                        return ConsensusStatus::Deciding;
                    },
                    ConsensusMessageKind::Commit => {
                        self.queue_commit(header, message);
                        return ConsensusStatus::Deciding;
                    },
                }
            },
            ProtoPhase::Preparing(i) => {
                // queue message if we're not preparing
                // or in the same seq as the message
                let i = match message.kind() {
                    ConsensusMessageKind::PrePrepare(_) => {
                        self.queue_pre_prepare(header, message);
                        return ConsensusStatus::Deciding;
                    },
                    ConsensusMessageKind::Prepare if message.sequence_number() != self.sequence_number() => {
                        self.queue_prepare(header, message);
                        return ConsensusStatus::Deciding;
                    },
                    ConsensusMessageKind::Prepare => i + 1,
                    ConsensusMessageKind::Commit => {
                        self.queue_commit(header, message);
                        return ConsensusStatus::Deciding;
                    },
                };
                // add message to the log
                log.insert(header, SystemMessage::Consensus(message));
                // check if we have gathered enough votes,
                // and transition to a new phase
                self.phase = if i == view.params().quorum() {
                    let message = SystemMessage::Consensus(ConsensusMessage::new(
                        self.sequence_number(),
                        ConsensusMessageKind::Commit,
                    ));
                    let targets = NodeId::targets(0..view.params().n());
                    node.broadcast(message, targets);
                    ProtoPhase::Committing(0)
                } else {
                    ProtoPhase::Preparing(i)
                };
                ConsensusStatus::Deciding
            },
            ProtoPhase::Committing(i) => {
                // queue message if we're not committing
                // or in the same seq as the message
                let i = match message.kind() {
                    ConsensusMessageKind::PrePrepare(_) => {
                        self.queue_pre_prepare(header, message);
                        return ConsensusStatus::Deciding;
                    },
                    ConsensusMessageKind::Prepare => {
                        self.queue_prepare(header, message);
                        return ConsensusStatus::Deciding;
                    },
                    ConsensusMessageKind::Commit if message.sequence_number() != self.sequence_number() => {
                        self.queue_commit(header, message);
                        return ConsensusStatus::Deciding;
                    },
                    ConsensusMessageKind::Commit => i + 1,
                };
                // add message to the log
                log.insert(header, SystemMessage::Consensus(message));
                // check if we have gathered enough votes,
                // and transition to a new phase
                if i == view.params().quorum() {
                    // we have reached a decision,
                    // notify core protocol
                    self.phase = ProtoPhase::Init;
                    ConsensusStatus::Decided(&self.current[..self.batch_size])
                } else {
                    self.phase = ProtoPhase::Committing(i);
                    ConsensusStatus::Deciding
                }
            },
        }
    }
}

impl<S> Deref for Consensus<S>
where
    S: Service + Send + 'static,
    State<S>: Send + Clone + 'static,
    Request<S>: Send + 'static,
    Reply<S>: Send + 'static,
{
    type Target = TboQueue;

    #[inline]
    fn deref(&self) -> &TboQueue {
        &self.tbo
    }
}

impl<S> DerefMut for Consensus<S>
where
    S: Service + Send + 'static,
    State<S>: Send + Clone + 'static,
    Request<S>: Send + 'static,
    Reply<S>: Send + 'static,
{
    #[inline]
    fn deref_mut(&mut self) -> &mut TboQueue {
        &mut self.tbo
    }
}
