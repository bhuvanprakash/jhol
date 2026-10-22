function counter() { let c = 0; return function() { c++; return c; }; }
const next = counter();
console.log(next());
console.log(next());
console.log(next());
let fns = [];
for (let i = 0; i < 3; i++) { fns.push(function() { return i; }); }
console.log(fns[0]());
console.log(fns[1]());
console.log(fns[2]());
function addOffset(o) { return function(x) { return x + o; }; }
const a = addOffset(100);
const b = addOffset(200);
console.log(a(1) + b(1));
