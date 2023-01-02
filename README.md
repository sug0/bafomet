Evil Byzantine fault tolerant middleware library/framework!

This project forks an early version of [FeBFT](https://github.com/SecureSolutionsLab/febft).

---

# A bit of context

`bafomet` is an efficient BFT SMR middleware library implementation, directly descendant
of protocols such as PBFT and BFT-SMaRt, where a static group of `n = 3f + 1` nodes
are responsible for replicating a service, that is usually exposed via a RPC interface.
The properties of these systems are such that despite the failure of (up to) `f` nodes
(due to software bugs, power outages, malicious attackers, etc) the service abstraction
will continue operating as usual.

Different from prior art in this field, usually implemented in Java, Rust was
the language of choice to implement all the typical SMR sub-protocols present
in `bafomet`. Many people are (rightfully so!) excited about the use of Rust
in new (and even older) software, because of its safety properties associated
with the many compile time checks present in the compiler, that are able to
hunt down common use-after-free as well as concurrency related bugs.

There are infinitely many use cases for BFT systems, which will undoubtedly improve the
availability of a digital service. However, a less robust class of systems, called CFT
systems, are often utilized in place of BFT systems, based on their greater performance.
Despite this, with the evolution of hardware, and especially the growing popularity of
blockchain technology, BFT systems are becoming more attractive to distributed system
developers.

People who are interested in the inner workings of these protocols can
consult the following papers:

* Castro, Miguel, and Barbara Liskov. "Practical Byzantine fault tolerance and proactive recovery." ACM Transactions on Computer Systems (TOCS) 20.4 (2002): 398-461.
* Bessani, Alysson, Joao Sousa, and Eduardo EP Alchieri. "State machine replication for the masses with BFT-SMART." 2014 44th Annual IEEE/IFIP International Conference on Dependable Systems and Networks. IEEE, 2014.

<!-- TODO: include link to thesis -->
To read more about the architecture of `bafomet`, a MsC thesis describing it
in detail is available upon request. Please contact `t1ag0_ at outlook.com`
if you want a copy.

# How to use this library?

Generally, to use this library, you will need to implement the following trait:

```rust
pub trait Service {
    /// The data types used by the application and the SMR protocol.
    ///
    /// This includes their respective serialization routines.
    type Data: SharedData;

    /// Returns the initial state of the application.
    fn initial_state(&mut self) -> Result<State<Self>>;

    /// Process a user request, producing a matching reply,
    /// meanwhile updating the application state.
    fn update(
        &mut self,
        state: &mut State<Self>,
        request: Request<Self>,
    ) -> Reply<Self>;
}
```

You may want to check out [client-local.rs](examples/client-local.rs) and
[replica-local.rs](examples/replica-local.rs) for examples of how to write
services utilizing `bafomet`. Run them with:

```
# Start the service replicas in a terminal window
$ cargo run --release --example replica-local

# In another terminal window, start the client(s)
$ cargo run --release --example client-local
```

# For contributors

The code is organized as follows:

* `src/bft/core/client` is the main entry point for a client.
* `src/bft/core/server` is the main entry point for a replica.
* `src/bft/consensus` implements the normal phase consensus code.
* `src/bft/consensus/log` implements the message log.
* `src/bft/sync` implements the view change code.
* `src/bft/cst` implements the state transfer code.
* `src/bft/communication` and its sub-modules implement the network code.
* `src/bft/executable` implements the thread responsible for running the
  user's application code.
* `src/bft/ordering` defines code for sequence numbers resistant to overflows.

Other modules should be somewhat self explanatory, especially if you read
the documentation generated with `cargo doc --features expose_impl` for `bafomet`.

# Licensing

Choose the MIT or Apache license at your option!
