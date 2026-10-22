// Non-ASCII must survive the lexer byte-exact: pushing one char per byte
// turned every multi-byte sequence into mojibake.
const hi = "héllo ✓";
console.log(hi);
const jp = "日本語テスト";
console.log(jp);
const emoji = "ok 👍";
console.log(emoji);
const t = `mixed é ${hi} 中`;
console.log(t);
const q = 'quote: " q';
console.log(q);
const esc = "tab\there";
console.log(esc);
const esc2 = "éscapé ✓";
console.log(esc2);
let ünïcode = 7;
console.log(ünïcode);
