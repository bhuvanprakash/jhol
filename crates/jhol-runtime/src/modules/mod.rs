use std::collections::HashMap;

pub mod buffer;
pub mod console;
pub mod crypto;
pub mod events;
pub mod fs;
pub mod http;
pub mod os;
pub mod path;
pub mod process;
pub mod timers;
pub mod util;

pub struct ModuleRegistry {
    modules: HashMap<String, Box<dyn BuiltinModule + Send + Sync>>,
}

pub trait BuiltinModule: Send + Sync {
    fn name(&self) -> &'static str;
    fn exports(&self) -> HashMap<String, String>;
}

impl ModuleRegistry {
    pub fn new() -> Self {
        let mut modules: HashMap<String, Box<dyn BuiltinModule + Send + Sync>> = HashMap::new();
        modules.insert("fs".to_string(), Box::new(fs::FsModule));
        modules.insert("path".to_string(), Box::new(path::PathModule));
        modules.insert("process".to_string(), Box::new(process::ProcessModule));
        modules.insert("buffer".to_string(), Box::new(buffer::BufferModule));
        modules.insert("os".to_string(), Box::new(os::OsModule));
        modules.insert("events".to_string(), Box::new(events::EventsModule));
        modules.insert("crypto".to_string(), Box::new(crypto::CryptoModule));
        modules.insert("http".to_string(), Box::new(http::HttpModule));
        modules.insert("console".to_string(), Box::new(console::ConsoleModule));
        modules.insert("timers".to_string(), Box::new(timers::TimersModule));
        modules.insert("util".to_string(), Box::new(util::UtilModule));
        Self { modules }
    }

    pub fn get(&self, name: &str) -> Option<&Box<dyn BuiltinModule + Send + Sync>> {
        self.modules.get(name)
    }

    pub fn has(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }

    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.modules.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }
}

pub fn module_source(name: &str, registry: &ModuleRegistry) -> Option<String> {
    let module = registry.get(name)?;
    let exports = module.exports();
    let mut source = String::new();

    source.push_str(&format!("var {name} = {{}}\n", name = name));

    for (export_name, export_value) in &exports {
        source.push_str(&format!(
            "{}['{}'] = {};\n",
            name, export_name, export_value
        ));
    }

    source.push_str(&format!("module.exports = {};\n", name));

    Some(source)
}

impl ModuleRegistry {
    pub fn get_exports(&self, name: &str) -> Option<String> {
        module_source(name, self)
    }
}
