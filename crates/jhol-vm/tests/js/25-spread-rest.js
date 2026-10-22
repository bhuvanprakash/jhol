const a = [1, 2, 3];
const b = [...a, 4, 5];
console.log(b.join(","));
console.log(Math.max(...a, 10));
const c = [...a, ...b];
console.log(c.join("-"));
function sum(...nums) { return nums.reduce((s, x) => s + x, 0); }
console.log(sum(1, 2, 3, 4));
function first(firstArg, ...restArgs) { return firstArg + ":" + restArgs.join(","); }
console.log(first("a", "b", "c"));
const chars = [..."abc"];
console.log(chars.join("+"));
const nums = [1, 2];
nums.push(...[3, 4]);
console.log(nums.join(","));
