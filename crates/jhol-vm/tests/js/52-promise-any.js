// Promise.any: first fulfilment wins, every rejection is collected, and
// all-reject produces an AggregateError whose `errors` keep input order.
// Empty input rejects, where all/allSettled/race fulfil with [].
const both = [Promise.reject(new Error("a")), Promise.reject(new Error("b"))];
Promise.any([Promise.reject(new Error("a")), Promise.resolve("ok")]).then((v) =>
  console.log("any", v)
);
Promise.any(both).catch((e) => {
  console.log("agg", e.name, e.errors.map((x) => x.message).join(","));
});
Promise.any([]).catch((e) => console.log("empty", e.name, e.errors.length));
