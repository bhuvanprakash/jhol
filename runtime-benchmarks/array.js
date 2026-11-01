var arr = [];
for (var i = 0; i < 1000000; i++) {
    arr.push(i);
}
var sum = 0;
for (var i = 0; i < arr.length; i++) {
    sum += arr[i];
}
if (sum !== 499999500000) {
    throw new Error("unexpected result: " + sum);
}
