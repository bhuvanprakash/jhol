let x = [[[[[[[[[[1]]]]]]]]]];
console.log(x[0][0][0][0][0][0][0][0][0][0]);
const t = { a: { b: { c: { d: { e: { f: 7 } } } } } };
console.log(t.a.b.c.d.e.f);
function f(a, b = a * 2, ...rest) { return [a, b, rest.length]; }
console.log(f(3), f(3, 4), f(3, 4, 5, 6));
const { a: { b: renamed = "dflt" } = {} } = {};
console.log(renamed);
let y = 0;
label1: for (let i = 0; i < 3; i++) { for (let j = 0; j < 3; j++) { if (j === 1) continue label1; y += 1; } }
console.log(y);
const ob = { async: 1, let: 2, class: 3, get: 4, set: 5, static: 6 };
console.log(ob.async + ob.let + ob.class + ob.get + ob.set + ob.static);
const tpl = `a${1 + 1}b`;
console.log(tpl, String.raw`raw\n` === "raw\\n");
