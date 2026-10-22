function makeCounter() { let count = 0; return function () { count++; return count; }; }
const inc = makeCounter();
let last = 0;
for (let i = 0; i < 1000000; i++) last = inc();
console.log(last);
