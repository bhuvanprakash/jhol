var obj = { a: 1, b: "hello", c: [1, 2, 3], d: { nested: true } };
for (var i = 0; i < 100000; i++) {
    var text = JSON.stringify(obj);
    var parsed = JSON.parse(text);
    if (parsed.a !== 1) {
        throw new Error("unexpected value");
    }
}
