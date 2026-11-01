var obj = {};
for (var i = 0; i < 1000000; i++) {
    obj["key" + i] = i;
}
var sum = 0;
for (var i = 0; i < 1000000; i++) {
    sum += obj["key" + i];
}
if (sum !== 499999500000) {
    throw new Error("unexpected result: " + sum);
}
