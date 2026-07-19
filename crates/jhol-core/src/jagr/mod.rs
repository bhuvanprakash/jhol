//! JAGR resolver — stub for compatibility.
pub struct JagrResolver;
impl JagrResolver {
    pub fn new() -> Self { Self }
    pub fn with_lp(self, _lp: bool) -> Self { self }
    pub fn with_timeout(self, _ms: u64) -> Self { self }
    pub fn with_skip_layer3(self, _skip: bool) -> Self { self }
    pub fn with_skip_layer2(self, _skip: bool) -> Self { self }
    pub fn with_run_all_layers(self, _all: bool) -> Self { self }
}
