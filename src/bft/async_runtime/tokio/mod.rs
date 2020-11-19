pub type JoinHandle<T> = ::tokio::task::JoinHandle<T>;

pub type Runtime = ::tokio::runtime::Runtime;

pub fn init(num_threads: usize) -> Result<Runtime, ()> {
    ::tokio::runtime::Builder::new_multi_thread()
        .worker_threads(num_threads)
        .thread_name("tokio-worker")
        .thread_stack_size(2 * 1024 * 1024)
        .enable_all()
        .build()
        .map_err(|_| ())
}