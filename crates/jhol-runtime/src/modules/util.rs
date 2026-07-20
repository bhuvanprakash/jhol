use super::BuiltinModule;
use std::collections::HashMap;

pub struct UtilModule;

impl BuiltinModule for UtilModule {
    fn name(&self) -> &'static str {
        "util"
    }

    fn exports(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();

        // inspect
        m.insert("inspect".to_string(),
            "(obj, options) => { \
             const depth = (options && options.depth) || 2; \
             const colors = (options && options.colors) || false; \
             if (obj === null) return 'null'; \
             if (obj === undefined) return 'undefined'; \
             if (typeof obj === 'string') return `'${obj}'`; \
             if (typeof obj === 'number') return String(obj); \
             if (typeof obj === 'boolean') return String(obj); \
             if (typeof obj === 'function') return `[Function: ${obj.name || '(anonymous)'}]`; \
             if (Array.isArray(obj)) { \
               if (depth <= 0) return '[Array]'; \
               const items = obj.map(i => util.inspect(i, { ...options, depth: depth - 1 })); \
               return `[ ${items.join(', ')} ]`; \
             } \
             if (typeof obj === 'object') { \
               if (depth <= 0) return '[Object]'; \
               const keys = Object.keys(obj); \
               const items = keys.map(k => `${k}: ${util.inspect(obj[k], { ...options, depth: depth - 1 })}`); \
               return `{ ${items.join(', ')} }`; \
             } \
             return String(obj); \
           }".to_string()
        );

        // format (printf-like)
        m.insert("format".to_string(),
            "(format, ...args) => { \
             if (typeof format !== 'string') return args.map(a => util.inspect(a)).join(' '); \
             let i = 0; \
             return format.replace(/%[sdifjoO%]/g, (match) => { \
               if (match === '%%') return '%'; \
               if (i >= args.length) return match; \
               const val = args[i++]; \
               switch (match) { \
                 case '%s': return String(val); \
                 case '%d': case '%i': return parseInt(val) || 0; \
                 case '%f': return parseFloat(val) || 0; \
                 case '%j': return JSON.stringify(val); \
                 case '%o': case '%O': return util.inspect(val); \
                 default: return match; \
               } \
             }) + (i < args.length ? ' ' + args.slice(i).map(a => util.inspect(a)).join(' ') : ''); \
           }".to_string()
        );

        // promisify
        m.insert(
            "promisify".to_string(),
            "(original) => { \
             function promisified(...args) { \
               return new Promise((resolve, reject) => { \
                 original.call(this, ...args, (err, ...result) => { \
                   if (err) reject(err); \
                   else resolve(result.length > 1 ? result : result[0]); \
                 }); \
               }); \
             } \
             promisified._original = original; \
             promisified.custom = util.promisify.custom; \
             return promisified; \
           }"
            .to_string(),
        );

        m.insert(
            "promisify.custom".to_string(),
            "Symbol('util.promisify.custom')".to_string(),
        );

        // callbackify
        m.insert("callbackify".to_string(),
            "(original) => { \
             function callbackified(...args) { \
               const cb = args.pop(); \
               if (typeof cb !== 'function') throw new TypeError('last argument must be a callback'); \
               original.apply(this, args).then(result => cb(null, result), err => cb(err)); \
             } \
             return callbackified; \
           }".to_string()
        );

        // types
        m.insert("types".to_string(),
            "{ \
             isDate: (v) => v instanceof Date, \
             isRegExp: (v) => v instanceof RegExp, \
             isArrayBuffer: (v) => v instanceof ArrayBuffer, \
             isSet: (v) => v instanceof Set, \
             isMap: (v) => v instanceof Map, \
             isPromise: (v) => v instanceof Promise, \
             isNativeError: (v) => v instanceof Error, \
             isArgumentsObject: (v) => false, \
             isAsyncFunction: (v) => typeof v === 'function' && v.constructor.name === 'AsyncFunction', \
             isGeneratorFunction: (v) => typeof v === 'function' && v.constructor.name === 'GeneratorFunction', \
             isGeneratorObject: (v) => v && typeof v.next === 'function', \
             isBooleanObject: (v) => v instanceof Boolean, \
             isNumberObject: (v) => v instanceof Number, \
             isStringObject: (v) => v instanceof String, \
             isSymbolObject: (v) => v instanceof Symbol, \
             isTypedArray: (v) => ArrayBuffer.isView(v) && !(v instanceof DataView), \
             isUint8Array: (v) => v instanceof Uint8Array, \
             isDataView: (v) => v instanceof DataView, \
             isSharedArrayBuffer: (v) => v instanceof SharedArrayBuffer, \
             isWeakSet: (v) => v instanceof WeakSet, \
             isWeakMap: (v) => v instanceof WeakMap, \
             isBoxedPrimitive: (v) => v instanceof Boolean || v instanceof Number || v instanceof String || v instanceof Symbol, \
             isAnyArrayBuffer: (v) => v instanceof ArrayBuffer || v instanceof SharedArrayBuffer, \
             isModuleNamespaceObject: (v) => false, \
             isProxy: (v) => false, \
             isExternal: (v) => false, \
           }".to_string()
        );

        // deprecate
        m.insert(
            "deprecate".to_string(),
            "(fn, msg) => { \
             let warned = false; \
             return (...args) => { \
               if (!warned) { process.emitWarning(msg, 'DeprecationWarning'); warned = true; } \
               return fn(...args); \
             }; \
           }"
            .to_string(),
        );

        // inherits
        m.insert("inherits".to_string(),
            "(ctor, superCtor) => { \
             if (ctor === undefined || ctor === null) throw new TypeError('The constructor to inherits must not be null or undefined'); \
             Object.setPrototypeOf(ctor.prototype, superCtor.prototype); \
             ctor.super_ = superCtor; \
             ctor.prototype[Symbol.species] = ctor; \
           }".to_string()
        );

        // debuglog
        m.insert(
            "debuglog".to_string(),
            "(section) => { \
             const logger = (...args) => { \
               if (process.env.NODE_DEBUG && process.env.NODE_DEBUG.includes(section)) { \
                 process.stderr.write(`${section} ${Date.now()}: ${args.join(' ')}\\n`); \
               } \
             }; \
             logger.enabled = (process.env.NODE_DEBUG || '').includes(section); \
             return logger; \
           }"
            .to_string(),
        );

        // getSystemErrorMap
        m.insert(
            "getSystemErrorMap".to_string(),
            "() => new Map()".to_string(),
        );

        // getSystemErrorName
        m.insert(
            "getSystemErrorName".to_string(),
            "(err) => `SystemError(${err})`".to_string(),
        );

        // toUSVString
        m.insert(
            "toUSVString".to_string(),
            "(str) => String(str).replace(/[\\uD800-\\uDFFF]/g, '')".to_string(),
        );

        // stripVTControlCharacters
        m.insert(
            "stripVTControlCharacters".to_string(),
            "(str) => String(str).replace(/\\x1b\\[[0-9;]*[a-zA-Z]/g, '')".to_string(),
        );

        // MIME type
        m.insert("MIMEType".to_string(),
            "class MIMEType { constructor(str) { this._str = str; } toString() { return this._str; } }".to_string()
        );
        m.insert(
            "MIMEParams".to_string(),
            "class MIMEParams { constructor() { this._params = {}; } }".to_string(),
        );

        // parseArgs
        m.insert(
            "parseArgs".to_string(),
            "(config) => { return { values: {}, positionals: [] }; }".to_string(),
        );

        // styleText
        m.insert(
            "styleText".to_string(),
            "(format, text) => String(text)".to_string(),
        );

        // transferable
        m.insert("transferable".to_string(), "() => {}".to_string());

        // abortable
        m.insert("abortable".to_string(),
            "(fn, signal) => (...args) => { if (signal && signal.aborted) throw new Error('Aborted'); return fn(...args); }".to_string()
        );

        m
    }
}
