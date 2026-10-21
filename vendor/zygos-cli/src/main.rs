use std::time::Duration;

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Tokio runtime");
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let code=runtime.block_on(async {
        tokio::select! {
            code=zygos_cli::run(&args)=>code,
            _=interrupted()=> {
                let error=zygos_cli::CliError::new("service_unavailable","operation interrupted; a sent mutation may already have committed");
                let mut stderr:zygos_cli::io::Output=Box::new(tokio::io::stderr());
                let _=zygos_cli::io::emit(&mut stderr,&error.envelope,false).await;
                130
            }
        }
    });
    // Tokio stdin may hold a blocking read after cancellation; do not wait forever.
    runtime.shutdown_timeout(Duration::from_millis(100));
    std::process::exit(code);
}
async fn interrupted() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
