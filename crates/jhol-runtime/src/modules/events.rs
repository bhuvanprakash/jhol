use super::BuiltinModule;
use std::collections::HashMap;

pub struct EventsModule;

impl BuiltinModule for EventsModule {
    fn name(&self) -> &'static str {
        "events"
    }

    fn exports(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();

        m.insert("EventEmitter".to_string(),
            "class EventEmitter { \
             constructor() { this._events = {}; this._maxListeners = 10; } \
             _getEvents() { return this._events; } \
             addListener(type, listener) { return this.on(type, listener); } \
             on(type, listener) { \
               if (!this._events[type]) this._events[type] = []; \
               this._events[type].push(listener); \
               return this; \
             } \
             once(type, listener) { \
               const wrapper = (...args) => { this.removeListener(type, wrapper); listener(...args); }; \
               wrapper._original = listener; \
               return this.on(type, wrapper); \
             } \
             prependListener(type, listener) { \
               if (!this._events[type]) this._events[type] = []; \
               this._events[type].unshift(listener); \
               return this; \
             } \
             prependOnceListener(type, listener) { \
               const wrapper = (...args) => { this.removeListener(type, wrapper); listener(...args); }; \
               wrapper._original = listener; \
               return this.prependListener(type, wrapper); \
             } \
             removeListener(type, listener) { \
               if (!this._events[type]) return this; \
               const idx = this._events[type].indexOf(listener); \
               if (idx !== -1) this._events[type].splice(idx, 1); \
               return this; \
             } \
             off(type, listener) { return this.removeListener(type, listener); } \
             removeAllListeners(type) { \
               if (type) delete this._events[type]; \
               else this._events = {}; \
               return this; \
             } \
             emit(type, ...args) { \
               if (!this._events[type]) return false; \
               const listeners = [...this._events[type]]; \
               for (const listener of listeners) { listener(...args); } \
               return true; \
             } \
             eventNames() { return Object.keys(this._events); } \
             listenerCount(type) { return this._events[type] ? this._events[type].length : 0; } \
             listeners(type) { return this._events[type] ? [...this._events[type]] : []; } \
             rawListeners(type) { return this._events[type] ? [...this._events[type]] : []; } \
             getMaxListeners() { return this._maxListeners; } \
             setMaxListeners(n) { this._maxListeners = n; return this; } \
             static defaultMaxListeners = 10; \
             static listenerCount(emitter, type) { return emitter.listenerCount(type); } \
             static errorMonitor = Symbol('events.errorMonitor'); \
           }".to_string()
        );

        // Once wrapper
        m.insert(
            "once".to_string(),
            "(emitter, name) => new Promise((resolve) => emitter.once(name, resolve))".to_string(),
        );

        // on (AsyncIterator)
        m.insert("on".to_string(),
            "(emitter, name) => ({ [Symbol.asyncIterator]() { return { next() { return new Promise(resolve => emitter.once(name, (...args) => resolve({ value: args, done: false }))); } } } })".to_string()
        );

        // getEventListeners (experimental)
        m.insert(
            "getEventListeners".to_string(),
            "(emitter, type) => emitter.listeners(type)".to_string(),
        );

        // errorMonitor
        m.insert(
            "errorMonitor".to_string(),
            "Symbol('events.errorMonitor')".to_string(),
        );

        // captureRejectionSymbol
        m.insert("captureRejections".to_string(), "false".to_string());
        m.insert(
            "captureRejectionSymbol".to_string(),
            "Symbol('events.captureRejection')".to_string(),
        );

        m
    }
}
