use super::BuiltinModule;
use std::collections::HashMap;

pub struct CryptoModule;

impl BuiltinModule for CryptoModule {
    fn name(&self) -> &'static str {
        "crypto"
    }

    fn exports(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();

        // Hash algorithms
        m.insert("createHash".to_string(),
            "(algorithm) => { \
             const algo = algorithm.toLowerCase(); \
             const validAlgos = ['sha256', 'sha224', 'sha384', 'sha512', 'sha1', 'md5', 'ripemd160', 'sha3-256', 'sha3-384', 'sha3-512']; \
             if (!validAlgos.includes(algo)) throw new Error(`Digest method not supported: ${algorithm}`); \
             let data = new Uint8Array(0); \
             return { \
               update(chunk, encoding) { \
                 if (typeof chunk === 'string') { data = Buffer.concat([data, Buffer.from(chunk, encoding || 'utf8')]); } \
                 else if (chunk instanceof Uint8Array) { data = Buffer.concat([data, chunk]); } \
                 return this; \
               }, \
               digest(encoding) { \
                 const hash = _jhol_hash(algo, data); \
                 if (encoding === 'hex') return hash; \
                 if (encoding === 'base64') return Buffer.from(hash, 'hex').toString('base64'); \
                 return Buffer.from(hash, 'hex'); \
               }, \
               copy() { const h = createHash(algo); h.update(data); return h; } \
             }; \
           }".to_string()
        );

        // HMAC
        m.insert("createHmac".to_string(),
            "(algorithm, key) => { \
             const algo = algorithm.toLowerCase(); \
             let data = new Uint8Array(0); \
             const keyBytes = typeof key === 'string' ? Buffer.from(key) : (key instanceof Uint8Array ? key : Buffer.from(String(key))); \
             return { \
               update(chunk) { \
                 if (typeof chunk === 'string') data = Buffer.concat([data, Buffer.from(chunk)]); \
                 else if (chunk instanceof Uint8Array) data = Buffer.concat([data, chunk]); \
                 return this; \
               }, \
               digest(encoding) { \
                 const hmac = _jhol_hmac(algo, keyBytes, data); \
                 if (encoding === 'hex') return hmac; \
                 if (encoding === 'base64') return Buffer.from(hmac, 'hex').toString('base64'); \
                 return Buffer.from(hmac, 'hex'); \
               } \
             }; \
           }".to_string()
        );

        // Random bytes
        m.insert(
            "randomBytes".to_string(),
            "(size, callback) => { \
             const bytes = new Uint8Array(size); \
             for (let i = 0; i < size; i++) bytes[i] = Math.floor(Math.random() * 256); \
             if (typeof callback === 'function') callback(null, bytes); \
             return bytes; \
           }"
            .to_string(),
        );

        // Random UUID
        m.insert(
            "randomUUID".to_string(),
            "(options) => { \
             const hex = '0123456789abcdef'; \
             let uuid = ''; \
             for (let i = 0; i < 36; i++) { \
               if (i === 8 || i === 13 || i === 18 || i === 23) uuid += '-'; \
               else if (i === 14) uuid += '4'; \
               else if (i === 19) uuid += (Math.floor(Math.random() * 4) + 8).toString(16); \
               else uuid += hex[Math.floor(Math.random() * 16)]; \
             } \
             return uuid; \
           }"
            .to_string(),
        );

        // RandomInt
        m.insert(
            "randomInt".to_string(),
            "(min, max) => { \
             if (max === undefined) { max = min; min = 0; } \
             return Math.floor(Math.random() * (max - min)) + min; \
           }"
            .to_string(),
        );

        // Cipher
        m.insert("createCipheriv".to_string(),
            "(algorithm, key, iv) => { throw new Error('Cipher not implemented in Jhol runtime'); }".to_string()
        );
        m.insert("createDecipheriv".to_string(),
            "(algorithm, key, iv) => { throw new Error('Decipher not implemented in Jhol runtime'); }".to_string()
        );

        // PBKDF2
        m.insert(
            "pbkdf2Sync".to_string(),
            "(password, salt, iterations, keylen, digest) => { \
             let hash = password + String(salt); \
             for (let i = 0; i < iterations; i++) { \
               hash = _jhol_hash(digest || 'sha256', Buffer.from(hash)); \
             } \
             return Buffer.from(hash.slice(0, keylen * 2), 'hex'); \
           }"
            .to_string(),
        );

        m.insert(
            "pbkdf2".to_string(),
            "(password, salt, iterations, keylen, digest, callback) => { \
             if (typeof callback !== 'function') throw new Error('Callback required'); \
             try { \
               const result = crypto.pbkdf2Sync(password, salt, iterations, keylen, digest); \
               callback(null, result); \
             } catch(e) { callback(e); } \
           }"
            .to_string(),
        );

        // Scrypt
        m.insert("scryptSync".to_string(),
            "(password, salt, keylen, options) => { return crypto.pbkdf2Sync(password, salt, 16384, keylen, 'sha256'); }".to_string()
        );
        m.insert("scrypt".to_string(),
            "(password, salt, keylen, options, callback) => { \
             if (typeof callback !== 'function') { callback = options; } \
             try { const r = crypto.scryptSync(password, salt, keylen, options); callback(null, r); } catch(e) { callback(e); } \
           }".to_string()
        );

        // Timing safe equal
        m.insert(
            "timingSafeEqual".to_string(),
            "(a, b) => { \
             if (a.length !== b.length) return false; \
             let result = 0; \
             for (let i = 0; i < a.length; i++) result |= a[i] ^ b[i]; \
             return result === 0; \
           }"
            .to_string(),
        );

        // Constants
        m.insert("constants".to_string(),
            "JSON.parse('{\"DH_CHECK_P_NOT_SAFE_PRIME\":2,\"DH_CHECK_P_NOT_PRIME\":1,\"DH_UNABLE_TO_CHECK_GENERATOR\":4,\"DH_NOT_SUITABLE_GENERATOR\":8,\"ALPN_ENABLED\":1,\"RSA_PKCS1_PADDING\":1,\"RSA_SSLV23_PADDING\":2,\"RSA_NO_PADDING\":3,\"RSA_PKCS1_OAEP_PADDING\":4,\"RSA_X931_PADDING\":5,\"RSA_PKCS1_PSS_PADDING\":6,\"RSA_PSS_SALTLEN_DIGEST\":-1,\"RSA_PSS_SALTLEN_MAX_SIGN\":-2,\"RSA_PSS_SALTLEN_AUTO\":-2,\"POINT_CONVERSION_COMPRESSED\":2,\"POINT_CONVERSION_UNCOMPRESSED\":4,\"POINT_CONVERSION_HYBRID\":6}')".to_string()
        );

        // Web Crypto API
        m.insert("subtle".to_string(), "undefined".to_string());
        m.insert("webcrypto".to_string(), "undefined".to_string());

        // Hash types
        m.insert("getHashes".to_string(),
            "() => ['sha1','sha224','sha256','sha384','sha512','md5','ripemd160','sha3-256','sha3-384','sha3-512']".to_string()
        );
        m.insert("getCiphers".to_string(),
            "() => ['aes-128-cbc','aes-192-cbc','aes-256-cbc','aes-128-gcm','aes-192-gcm','aes-256-gcm']".to_string()
        );
        m.insert(
            "getCurves".to_string(),
            "() => ['prime256v1','secp384r1','secp521r1','X25519','X448','Ed25519','Ed448']"
                .to_string(),
        );

        // Certificate
        m.insert("Certificate".to_string(),
            "() => ({ exportChallenge: () => {}, verifyChallenge: () => {}, verifySpkac: () => {} })".to_string()
        );

        // DiffieHellman
        m.insert("DiffieHellman".to_string(),
            "class DiffieHellman { constructor() { throw new Error('DiffieHellman not implemented'); } }".to_string()
        );
        m.insert("DiffieHellmanGroup".to_string(),
            "class DiffieHellmanGroup { constructor() { throw new Error('DiffieHellmanGroup not implemented'); } }".to_string()
        );
        m.insert(
            "ECDH".to_string(),
            "class ECDH { constructor() { throw new Error('ECDH not implemented'); } }".to_string(),
        );

        // Key generation
        m.insert(
            "generateKeyPairSync".to_string(),
            "(type, options) => { throw new Error('Key generation not implemented'); }".to_string(),
        );
        m.insert("generateKeyPair".to_string(),
            "(type, options, callback) => { if (typeof callback === 'function') callback(new Error('Not implemented')); }".to_string()
        );

        // X509Certificate
        m.insert("X509Certificate".to_string(),
            "class X509Certificate { constructor() { throw new Error('X509Certificate not implemented'); } }".to_string()
        );

        // Secure random
        m.insert(
            "randomFillSync".to_string(),
            "(buffer, offset, size) => { \
             const off = offset || 0; const sz = size || buffer.length - off; \
             for (let i = 0; i < sz; i++) buffer[off + i] = Math.floor(Math.random() * 256); \
             return buffer; \
           }"
            .to_string(),
        );
        m.insert("randomFill".to_string(),
            "(buffer, offset, size, callback) => { \
             if (typeof callback !== 'function') { callback = size; size = undefined; } \
             try { crypto.randomFillSync(buffer, offset, size); callback(null, buffer); } catch(e) { callback(e); } \
           }".to_string()
        );

        // FIPS
        m.insert("getFips".to_string(), "() => false".to_string());
        m.insert("setFips".to_string(), "(fips) => {}".to_string());
        m.insert(
            "checkPrime".to_string(),
            "(n, callback) => { if (typeof callback === 'function') callback(null, true); }"
                .to_string(),
        );
        m.insert("checkPrimeSync".to_string(), "() => true".to_string());
        m.insert("generatePrime".to_string(),
            "(size, options, callback) => { if (typeof callback === 'function') callback(new Error('Not implemented')); }".to_string()
        );

        m
    }
}
