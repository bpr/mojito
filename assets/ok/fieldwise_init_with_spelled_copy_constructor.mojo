# A copy constructor spelled `__init__(out self, *, copy: Self)` is the
# struct's copy constructor, not a hand-written initializer, so a
# `@fieldwise_init` struct declaring one keeps its synthesized fieldwise
# constructor, keywords included.
@fieldwise_init
struct P(Copyable):
    var x: Int
    var y: Int

    def __init__(out self, *, copy: Self):
        self.x = copy.x
        self.y = copy.y


def main():
    var p = P(y=2, x=1)
    var q = p.copy()
    print(q.x, q.y)
