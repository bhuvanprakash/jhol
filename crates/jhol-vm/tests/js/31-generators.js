function* gen() { yield 1; yield 2; yield 3; }
const g = gen();
console.log(g.next().value);
console.log(g.next().value);
console.log(g.next().value);
console.log(g.next().done);
function* counter() { yield "a"; yield "b"; }
console.log([...counter()].join(","));
for (const v of gen()) console.log(v);
