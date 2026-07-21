#[cfg(feature = "js-engine")]
pub mod js_engine_backend {
    use js_engine::JSEngine;

    pub struct JSEngineBackend {
        engine: JSEngine,
    }

    impl JSEngineBackend {
        pub fn new() -> Self {
            Self {
                engine: JSEngine::new(),
            }
        }

        pub fn run_script(&mut self, source: &str) -> Result<String, String> {
            let result = self.engine.run_script(source, "<js-engine>");
            Ok(format!("{:?}", result))
        }

        pub fn run_file(&mut self, path: &str) -> Result<String, String> {
            let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            self.run_script(&source)
        }
    }
}
