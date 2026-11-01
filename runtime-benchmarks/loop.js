var sum = 0;
for (var i = 0; i < 10000000; i++) {
    sum += i;
}
if (sum !== 49999995000000) {
    throw new Error("unexpected result: " + sum);
}
