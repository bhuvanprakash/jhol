const obj = { add: function (x, y) { return x + y; }, mul: function (x, y) { return x * y; } };
let sum = 0;
for (let i = 0; i < 1000000; i++) { if ((i & 1) === 0) sum += obj.add(i, 3); else sum += obj.mul(i, 2); }
console.log(sum);
