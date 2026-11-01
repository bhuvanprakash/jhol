const o = {};
o["__proto__"].x = 1;
console.log("polluted?", {}.x);
const p = JSON.parse('{"__proto__": {"polluted": 1}, "a": 1}');
console.log(Object.keys(p), p.a, ({}).polluted);
const q = {};
q.constructor = "c";
console.log(q.constructor, typeof q.toString);
const inherited = Object.create({ shared: 1 });
inherited.own = 2;
console.log(inherited.shared, inherited.own, "shared" in inherited, Object.keys(inherited).length);
delete Object.prototype;
try { console.log(({}).toString); } catch (e) { console.log("threw"); }
