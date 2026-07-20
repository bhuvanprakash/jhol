use super::BuiltinModule;
use std::collections::HashMap;

pub struct BufferModule;

impl BuiltinModule for BufferModule {
    fn name(&self) -> &'static str {
        "buffer"
    }

    fn exports(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();

        // Buffer.from
        m.insert("Buffer".to_string(),
            "class Buffer extends Uint8Array { \
             constructor(size) { super(size || 0); } \
             static from(data, encoding) { \
               if (typeof data === 'string') { \
                 const encoder = new TextEncoder(); \
                 return encoder.encode(data); \
               } \
               if (Array.isArray(data)) { \
                 return new Uint8Array(data); \
               } \
               if (data instanceof ArrayBuffer || ArrayBuffer.isView(data)) { \
                 return new Uint8Array(data); \
               } \
               if (typeof data === 'number') { \
                 return new Uint8Array(data); \
               } \
               return new Uint8Array(0); \
             } \
             static alloc(size, fill, encoding) { \
               const buf = new Uint8Array(size); \
               if (fill !== undefined && fill !== null) { \
                 if (typeof fill === 'number') buf.fill(fill); \
                 else if (typeof fill === 'string') { \
                   const enc = new TextEncoder(); \
                   const bytes = enc.encode(fill); \
                   for (let i = 0; i < size; i++) buf[i] = bytes[i % bytes.length]; \
                 } else if (fill instanceof Uint8Array) { \
                   for (let i = 0; i < size; i++) buf[i] = fill[i % fill.length]; \
                 } \
               } \
               return buf; \
             } \
             static allocUnsafe(size) { return new Uint8Array(size); } \
             static byteLength(string, encoding) { \
               if (typeof string === 'string') return new TextEncoder().encode(string).length; \
               if (ArrayBuffer.isView(string)) return string.byteLength; \
               return String(string).length; \
             } \
             static concat(list, totalLength) { \
               if (!Array.isArray(list)) throw new TypeError('list must be an array'); \
               const bufs = list.map(b => b instanceof Uint8Array ? b : new Uint8Array(b)); \
               const total = totalLength || bufs.reduce((s, b) => s + b.length, 0); \
               const result = new Uint8Array(total); \
               let offset = 0; \
               for (const buf of bufs) { result.set(buf, offset); offset += buf.length; } \
               return result; \
             } \
             static isBuffer(obj) { return obj instanceof Uint8Array || (obj && obj._isBuffer); } \
             static compare(a, b) { \
               for (let i = 0; i < Math.min(a.length, b.length); i++) { \
                 if (a[i] < b[i]) return -1; if (a[i] > b[i]) return 1; \
               } \
               return a.length - b.length; \
             } \
             static isEncoding(enc) { return ['utf8','utf-8','ascii','base64','hex','latin1','binary','ucs2','ucs-2','utf16le','utf-16le'].includes(String(enc).toLowerCase()); } \
             toString(encoding, start, end) { \
               const slice = this.slice(start || 0, end || this.length); \
               if (encoding === 'hex') return Array.from(slice).map(b => b.toString(16).padStart(2, '0')).join(''); \
               if (encoding === 'base64') { return btoa(String.fromCharCode(...slice)); } \
               return new TextDecoder('utf-8').decode(slice); \
             } \
             toJSON() { return { type: 'Buffer', data: Array.from(this) }; } \
             equals(other) { return Buffer.compare(this, other) === 0; } \
             copy(target, targetStart, sourceStart, sourceEnd) { \
               const tStart = targetStart || 0; const sStart = sourceStart || 0; \
               const sEnd = sourceEnd || this.length; const len = sEnd - sStart; \
               for (let i = 0; i < len; i++) target[tStart + i] = this[sStart + i]; \
               return len; \
             } \
             slice(start, end) { return this.subarray(start, end); } \
             readUInt8(offset) { return this[offset]; } \
             readUInt16LE(offset) { return this[offset] | (this[offset + 1] << 8); } \
             readUInt32LE(offset) { return this[offset] | (this[offset + 1] << 8) | (this[offset + 2] << 16) | (this[offset + 3] << 24); } \
             readInt8(offset) { const v = this[offset]; return v & 0x80 ? v - 256 : v; } \
             readInt16LE(offset) { const v = this.readUInt16LE(offset); return v & 0x8000 ? v - 65536 : v; } \
             readInt32LE(offset) { const v = this.readUInt32LE(offset); return v | 0; } \
             readFloatLE(offset) { const arr = new Uint8Array(4); for (let i = 0; i < 4; i++) arr[i] = this[offset + i]; return new Float32Array(arr.buffer)[0]; } \
             readDoubleLE(offset) { const arr = new Uint8Array(8); for (let i = 0; i < 8; i++) arr[i] = this[offset + i]; return new Float64Array(arr.buffer)[0]; } \
             write(string, offset, length, encoding) { const bytes = new TextEncoder().encode(String(string)); const off = offset || 0; for (let i = 0; i < bytes.length && i < (length || bytes.length); i++) this[off + i] = bytes[i]; return bytes.length; } \
             writeUInt8(value, offset) { this[offset] = value & 0xFF; } \
             writeUInt16LE(value, offset) { this[offset] = value & 0xFF; this[offset + 1] = (value >> 8) & 0xFF; } \
             writeUInt32LE(value, offset) { this[offset] = value & 0xFF; this[offset + 1] = (value >> 8) & 0xFF; this[offset + 2] = (value >> 16) & 0xFF; this[offset + 3] = (value >> 24) & 0xFF; } \
             writeInt32LE(value, offset) { this.writeUInt32LE(value >>> 0, offset); } \
             fill(value, offset, end) { const off = offset || 0; const e = end || this.length; for (let i = off; i < e; i++) this[i] = typeof value === 'number' ? value & 0xFF : (String(value).charCodeAt(0) & 0xFF); } \
             indexOf(value, byteOffset) { const off = byteOffset || 0; const v = typeof value === 'number' ? value : value instanceof Uint8Array ? value[0] : String(value).charCodeAt(0); for (let i = off; i < this.length; i++) if (this[i] === v) return i; return -1; } \
             includes(value, byteOffset) { return this.indexOf(value, byteOffset) !== -1; } \
             swap16() { for (let i = 0; i < this.length; i += 2) { const t = this[i]; this[i] = this[i + 1]; this[i + 1] = t; } return this; } \
             swap32() { for (let i = 0; i < this.length; i += 4) { const t0 = this[i]; const t1 = this[i + 1]; this[i] = this[i + 3]; this[i + 1] = this[i + 2]; this[i + 2] = t1; this[i + 3] = t0; } return this; } \
           }".to_string()
        );

        // SlowBuffer (deprecated)
        m.insert(
            "SlowBuffer".to_string(),
            "(size) => new Uint8Array(size)".to_string(),
        );

        // Constants
        m.insert("INSPECT_MAX_BYTES".to_string(), "50".to_string());
        m.insert("kMaxLength".to_string(), "4294967296".to_string());
        m.insert("kStringMaxLength".to_string(), "536870888".to_string());

        m
    }
}
