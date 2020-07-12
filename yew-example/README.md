1. rustup target add wasm32-unknown-unknown
2. cargo install wasm-pack
3. wasm-pack build --target web --out-name wasm --out-dir ./static
4. static serve `static` dir
5. Run local node from this branch which has port 8901 setup for ws connections
6. Open app web page and click "Get balance"

Note that logs are routed to console so use `log::info!` liberally :)
