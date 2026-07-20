use super::BuiltinModule;
use std::collections::HashMap;

pub struct HttpModule;

impl BuiltinModule for HttpModule {
    fn name(&self) -> &'static str {
        "http"
    }

    fn exports(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();

        // HTTP status codes
        m.insert("STATUS_CODES".to_string(),
            "JSON.parse('{\"100\":\"Continue\",\"101\":\"Switching Protocols\",\"102\":\"Processing\",\"103\":\"Early Hints\",\"200\":\"OK\",\"201\":\"Created\",\"202\":\"Accepted\",\"203\":\"Non-Authoritative Information\",\"204\":\"No Content\",\"205\":\"Reset Content\",\"206\":\"Partial Content\",\"207\":\"Multi-Status\",\"300\":\"Multiple Choices\",\"301\":\"Moved Permanently\",\"302\":\"Found\",\"303\":\"See Other\",\"304\":\"Not Modified\",\"305\":\"Use Proxy\",\"307\":\"Temporary Redirect\",\"308\":\"Permanent Redirect\",\"400\":\"Bad Request\",\"401\":\"Unauthorized\",\"402\":\"Payment Required\",\"403\":\"Forbidden\",\"404\":\"Not Found\",\"405\":\"Method Not Allowed\",\"406\":\"Not Acceptable\",\"407\":\"Proxy Authentication Required\",\"408\":\"Request Timeout\",\"409\":\"Conflict\",\"410\":\"Gone\",\"411\":\"Length Required\",\"412\":\"Precondition Failed\",\"413\":\"Payload Too Large\",\"414\":\"URI Too Long\",\"415\":\"Unsupported Media Type\",\"416\":\"Range Not Satisfiable\",\"417\":\"Expectation Failed\",\"418\":\"I\\'m a Teapot\",\"422\":\"Unprocessable Entity\",\"425\":\"Too Early\",\"426\":\"Upgrade Required\",\"428\":\"Precondition Required\",\"429\":\"Too Many Requests\",\"431\":\"Request Header Fields Too Large\",\"451\":\"Unavailable For Legal Reasons\",\"500\":\"Internal Server Error\",\"501\":\"Not Implemented\",\"502\":\"Bad Gateway\",\"503\":\"Service Unavailable\",\"504\":\"Gateway Timeout\",\"505\":\"HTTP Version Not Supported\",\"506\":\"Variant Also Negotiates\",\"507\":\"Insufficient Storage\",\"508\":\"Loop Detected\",\"509\":\"Bandwidth Limit Exceeded\",\"510\":\"Not Extended\",\"511\":\"Network Authentication Required\"}')".to_string()
        );

        // HTTP methods
        m.insert("METHODS".to_string(),
            "JSON.parse('[\"ACL\",\"BIND\",\"CHECKOUT\",\"CONNECT\",\"COPY\",\"DELETE\",\"GET\",\"HEAD\",\"LINK\",\"LOCK\",\"M-SEARCH\",\"MERGE\",\"MKACTIVITY\",\"MKCALENDAR\",\"MKCOL\",\"MOVE\",\"NOTIFY\",\"OPTIONS\",\"PATCH\",\"POST\",\"PROPFIND\",\"PROPPATCH\",\"PURGE\",\"PUT\",\"REBIND\",\"REPORT\",\"SEARCH\",\"SOURCE\",\"SUBSCRIBE\",\"TRACE\",\"UNBIND\",\"UNLINK\",\"UNLOCK\",\"UNSUBSCRIBE\"]')".to_string()
        );

        // createServer
        m.insert("createServer".to_string(),
            "(requestListener) => { \
             const EventEmitter = require('events').EventEmitter; \
             const server = new EventEmitter(); \
             server._listening = false; \
             server.address = () => ({ address: '0.0.0.0', family: 'IPv4', port: server._port || 0 }); \
             server.listen = (port, host, callback) => { \
               if (typeof port === 'function') { callback = port; port = 0; } \
               if (typeof host === 'function') { callback = host; host = '0.0.0.0'; } \
               server._port = typeof port === 'object' ? (port.port || 0) : (port || 0); \
               server._host = host || '0.0.0.0'; \
               server._listening = true; \
               server.emit('listening'); \
               if (callback) callback(); \
               return server; \
             }; \
             server.close = (callback) => { \
               server._listening = false; \
               server.emit('close'); \
               if (callback) callback(); \
               return server; \
             }; \
             server.timeout = 300000; \
             server.keepAliveTimeout = 5000; \
             server.headersTimeout = 60000; \
             server.maxHeadersCount = 2000; \
             if (requestListener) server.on('request', requestListener); \
             return server; \
           }".to_string()
        );

        // request (HTTP client)
        m.insert("request".to_string(),
            "(url, options, callback) => { \
             if (typeof url === 'object' && !options && !callback) { options = url; url = undefined; } \
             if (typeof url === 'string' || url instanceof URL) { \
               if (typeof options === 'function') { callback = options; options = {}; } \
               if (!options) options = {}; \
               if (typeof url === 'string') options.href = url; \
               else options.href = url.href; \
             } \
             if (typeof options === 'function') { callback = options; options = {}; } \
             if (!options) options = {}; \
             const EventEmitter = require('events').EventEmitter; \
             const req = new EventEmitter(); \
             req.aborted = false; \
             req.destroyed = false; \
             req.method = (options.method || 'GET').toUpperCase(); \
             req.path = options.path || options.href || '/'; \
             req.headers = options.headers || {}; \
             req.setHeader = (name, value) => { req.headers[name] = value; }; \
             req.getHeader = (name) => req.headers[name]; \
             req.removeHeader = (name) => { delete req.headers[name]; }; \
             req.getHeaders = () => ({...req.headers}); \
             req.hasHeader = (name) => name in req.headers; \
             req.flushHeaders = () => {}; \
             req.setNoDelay = () => {}; \
             req.setSocketKeepAlive = () => {}; \
             req.setTimeout = (ms, cb) => { if (cb) req.on('timeout', cb); return req; }; \
             req.write = (chunk) => { req._body = (req._body || '') + (typeof chunk === 'string' ? chunk : new TextDecoder().decode(chunk)); return true; }; \
             req.end = (data) => { \
               if (data) req.write(data); \
               req._ended = true; \
               if (!req._listening) { \
                 req.emit('response', { \
                   statusCode: 200, \
                   statusMessage: 'OK', \
                   headers: { 'content-type': 'text/plain', 'content-length': '0' }, \
                   headersDistinct: {}, \
                   httpVersion: '1.1', \
                   socket: {}, \
                   on: (evt, handler) => { req.on(evt, handler); }, \
                   setEncoding: () => {}, \
                   statusMessage: 'OK', \
                 }); \
               } \
             }; \
             if (callback) req.on('response', callback); \
             setTimeout(() => req.emit('response', { \
               statusCode: 200, \
               statusMessage: 'OK', \
               headers: {}, \
               httpVersion: '1.1', \
               socket: {}, \
               on: (evt, handler) => req.on(evt, handler), \
               setEncoding: () => {} \
             }), 0); \
             return req; \
           }".to_string()
        );

        // get (convenience wrapper around request)
        m.insert(
            "get".to_string(),
            "(url, options, callback) => { \
             if (typeof options === 'function') { callback = options; options = {}; } \
             const req = http.request(url, options, callback); \
             req.end(); \
             return req; \
           }"
            .to_string(),
        );

        // Server response class
        m.insert("ServerResponse".to_string(),
            "class ServerResponse { \
             constructor(req) { \
               this.req = req; \
               this.statusCode = 200; \
               this.statusMessage = 'OK'; \
               this._headers = {}; \
               this._headerSent = false; \
               this._chunks = []; \
               this.finished = false; \
               this.socket = {}; \
               this.headersSent = false; \
               this.sendDate = true; \
               this.strictContentLength = false; \
             } \
             setHeader(name, value) { this._headers[String(name).toLowerCase()] = value; } \
             getHeader(name) { return this._headers[String(name).toLowerCase()]; } \
             getHeaders() { return {...this._headers}; } \
             getHeaderNames() { return Object.keys(this._headers); } \
             hasHeader(name) { return String(name).toLowerCase() in this._headers; } \
             removeHeader(name) { delete this._headers[String(name).toLowerCase()]; } \
             addTrailers(headers) {} \
             writeHead(statusCode, statusMessage, headers) { \
               if (typeof statusMessage === 'object') { headers = statusMessage; statusMessage = undefined; } \
               this.statusCode = statusCode; \
               if (statusMessage) this.statusMessage = statusMessage; \
               if (headers) Object.assign(this._headers, headers); \
               this.headersSent = true; \
               return this; \
             } \
             write(chunk) { \
               const data = typeof chunk === 'string' ? chunk : new TextDecoder().decode(chunk); \
               this._chunks.push(data); \
               this._headerSent = true; \
               return true; \
             } \
             end(data, encoding, callback) { \
               if (typeof data === 'function') { callback = data; data = undefined; } \
               if (typeof encoding === 'function') { callback = encoding; encoding = undefined; } \
               if (data) this.write(data); \
               this.finished = true; \
               if (callback) callback(); \
               return this; \
             } \
             flushHeaders() {} \
             setTimeout(ms, cb) { if (cb) this.on('timeout', cb); return this; } \
             get statusCode() { return this._statusCode || 200; } \
             set statusCode(code) { this._statusCode = code; } \
             get statusMessage() { return this._statusMsg || 'OK'; } \
             set statusMessage(msg) { this._statusMsg = msg; } \
           }".to_string()
        );

        // IncomingMessage
        m.insert(
            "IncomingMessage".to_string(),
            "class IncomingMessage { \
             constructor(socket) { \
               this.socket = socket; \
               this._readableState = {}; \
               this.httpVersion = '1.1'; \
               this.httpVersionMajor = 1; \
               this.httpVersionMinor = 1; \
               this.complete = false; \
               this.rawHeaders = []; \
               this.rawTrailers = []; \
               this.headers = {}; \
               this.headersDistinct = {}; \
               this.trailers = {}; \
               this.trailersDistinct = {}; \
               this.statusCode = 200; \
               this.statusMessage = 'OK'; \
               this.method = 'GET'; \
               this.url = '/'; \
               this.socket = {}; \
               this.connection = {}; \
               this._body = ''; \
             } \
             setEncoding(encoding) {} \
             destroy(error) {} \
             read(size) { return null; } \
           }"
            .to_string(),
        );

        // maxHeaderSize
        m.insert("maxHeaderSize".to_string(), "16384".to_string());

        // globalAgent
        m.insert("globalAgent".to_string(),
            "{ maxSockets: Infinity, maxFreeSockets: 256, maxTotalSockets: Infinity, scheduling: 'lifo', keepAlive: true, keepAliveMsecs: 1000, timeout: 5000 }".to_string()
        );

        // Agent
        m.insert(
            "Agent".to_string(),
            "class Agent { \
             constructor(options) { \
               this.maxSockets = (options && options.maxSockets) || Infinity; \
               this.maxFreeSockets = (options && options.maxFreeSockets) || 256; \
               this.maxTotalSockets = (options && options.maxTotalSockets) || Infinity; \
               this.scheduling = (options && options.scheduling) || 'lifo'; \
               this.keepAlive = (options && options.keepAlive) || false; \
               this.keepAliveMsecs = (options && options.keepAliveMsecs) || 1000; \
               this.timeout = (options && options.timeout) || 5000; \
               this._sockets = {}; \
               this._freeSockets = {}; \
               this._requests = {}; \
             } \
             createConnection(options, cb) { \
               const socket = { destroy: () => {} }; \
               if (cb) cb(null, socket); \
               return socket; \
             } \
             destroy() {} \
           }"
            .to_string(),
        );

        m
    }
}
