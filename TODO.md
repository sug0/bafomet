# TODO

* fix clippy warnings
* tag `Node` type with `Client` or `Replica` marker types
* replace `globals` with something more secure,
  that doesn't require unsafe code
* add some sort of ABCI-like API?
* support non-uniform voting power, per replica,
  to bring us closer to a Tendermint-like weighted
  round-robin leader election?
    * perhaps even implement Tendermint's consensus
    protocol? or some variation of it, that still
    respects liveness and security properties
* add `tower::Service` adapter for `bafomet`'s service
  trait
* patch client code with fixes from `research` branch
    * fix client future - should only have 1 mutex
    * more changes??
* fix view change code, which had some bugs
* test CST code, which is probably buggy as all hell
    * implement actual CST algo?
* QOL things, like serializing state upon shutting down,
  etc
* socket connections should not hang on forever waiting
  for new data
* handle clients disconnecting
* organize log as a merkle tree, to be able to request
  arbitrary proofs?
* separate code into different sub-crates?
    * maybe `communication` code should just be a trait
      that can be implemented by the user? and we provide
      some impls for this trait.
    * try to make most things a trait? should remove feature
      flag spaghetti from the code
* remove batching code from replicas
    * new design: <https://u.sicp.me/ftndz.png>
* P2P network keeping track of the latest view; views may
  be updated across epochs; epochs change automatically
  every X consensus instances; replicas from the current
  epoch must sign the view of the next epoch, whose F
  parameter may be updated; this means that the current
  view must always know the view of the next epoch!
* batch sig verification
    * <https://github.com/dalek-cryptography/ed25519-dalek>
