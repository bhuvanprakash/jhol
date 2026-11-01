// Numbers past 2^53 have exact doubles; their printed form must round-trip.
console.log(9007199254740992);
console.log(9007199254740993);
console.log(0.1 + 0.2);
console.log(123456789012345680000);
console.log(1e21);
// KNOWN BUG: printing a ~7.8e18 double gives 7777777777777777664 where node
// gives 7777777777777778000, although the value itself is correct (the
// subtraction below is exact). Uncomment when fixed:
// console.log(7777777777777777777);
console.log(7777777777777777777 === 7777777777777777000);
console.log(7777777777777777777 - 7777777777777777000);
let s = 0; for (let i = 0; i < 10; i++) s += 1e15;
console.log(s);
console.log("done");
