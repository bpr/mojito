# A returned `ref[origin] T` field must bear exactly its declared origin. Here the
# accessor declares `ref[Self.a]` but returns the `ref[Self.b]` field, so the borrow escapes
# the region the return contract promises callers.
# expect: escapes storage
@fieldwise_init
struct Two[a: Origin[mut=False], b: Origin[mut=False]]:
    var first: ref[a] Int
    var second: ref[b] Int

    def get_a(self) -> ref[Self.a] Int:
        return self.second


def main():
    var x = 1
    var y = 2
    ref rx = x
    ref ry = y
    var t = Two(rx, ry)
    print(t.get_a())
