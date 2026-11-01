var s = "";
for (var i = 0; i < 100000; i++) {
    s += "x";
}
if (s.length !== 100000) {
    throw new Error("unexpected length: " + s.length);
}
