# Two destructuring targets formed through one struct's `ref` getter. Each
# leaf's reference names an origin and excludes nothing until its store, as
# upstream's lvalues do, so both stores run: `5 6`.
struct R:
    var a: Int
    var b: Int

    def __init__(out self):
        self.a = 0
        self.b = 0

    def __getitem__(ref self, i: Int) -> ref[origin_of(self.a, self.b)] Int:
        if i == 0:
            return self.a
        return self.b


def main():
    var r = R()
    (r[0], r[1]) = (5, 6)
    print(r.a, r.b)
