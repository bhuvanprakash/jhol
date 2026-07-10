use super::BuiltinModule;
use std::collections::HashMap;

pub struct TimersModule;

impl BuiltinModule for TimersModule {
    fn name(&self) -> &'static str {
        "timers"
    }

    fn exports(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();

        m.insert("setTimeout".to_string(),
            "(callback, delay, ...args) => { \
             if (typeof callback === 'string') callback = new Function(callback); \
             const id = _jhol_timer_id++; \
             _jhol_timers[id] = { type: 'timeout', callback, delay: delay || 1, args, fired: false }; \
             setTimeout(() => { \
               if (!_jhol_timers[id] || _jhol_timers[id].fired) return; \
               _jhol_timers[id].fired = true; \
               callback(...args); \
               delete _jhol_timers[id]; \
             }, delay || 1); \
             return id; \
           }".to_string()
        );

        m.insert("clearTimeout".to_string(),
            "(id) => { if (_jhol_timers && _jhol_timers[id]) { _jhol_timers[id].fired = true; delete _jhol_timers[id]; } }".to_string()
        );

        m.insert(
            "setInterval".to_string(),
            "(callback, delay, ...args) => { \
             if (typeof callback === 'string') callback = new Function(callback); \
             const id = _jhol_timer_id++; \
             _jhol_timers[id] = { type: 'interval', callback, delay: delay || 1, args }; \
             _jhol_intervals[id] = setInterval(() => { callback(...args); }, delay || 1); \
             return id; \
           }"
            .to_string(),
        );

        m.insert("clearInterval".to_string(),
            "(id) => { if (_jhol_intervals && _jhol_intervals[id]) { clearInterval(_jhol_intervals[id]); delete _jhol_intervals[id]; } if (_jhol_timers) delete _jhol_timers[id]; }".to_string()
        );

        m.insert(
            "setImmediate".to_string(),
            "(callback, ...args) => { \
             if (typeof callback === 'string') callback = new Function(callback); \
             const id = _jhol_timer_id++; \
             _jhol_timers[id] = { type: 'immediate', callback, args, fired: false }; \
             queueMicrotask(() => { \
               if (!_jhol_timers[id] || _jhol_timers[id].fired) return; \
               _jhol_timers[id].fired = true; \
               callback(...args); \
               delete _jhol_timers[id]; \
             }); \
             return id; \
           }"
            .to_string(),
        );

        m.insert("clearImmediate".to_string(),
            "(id) => { if (_jhol_timers && _jhol_timers[id]) { _jhol_timers[id].fired = true; delete _jhol_timers[id]; } }".to_string()
        );

        // active (deprecated)
        m.insert("active".to_string(), "(timer) => {}".to_string());

        // enroll (deprecated)
        m.insert("enroll".to_string(), "(item, msecs) => {}".to_string());
        m.insert("unenroll".to_string(), "(item) => {}".to_string());

        m
    }
}
