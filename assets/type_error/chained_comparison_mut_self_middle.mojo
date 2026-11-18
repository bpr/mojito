# A chained comparison's first middle operand is an rvalue for its second
# link: the copy taken after the first link. A `mut self` dunder cannot bind
# it, as upstream's call binding rejects any temporary passed to a mutable
# argument.
# expect: value passed to a mutable argument must be mutable
@fieldwise_init
struct S(ImplicitlyCopyable):
    var v: Int

    def __lt__(mut self, o: Self) -> Bool:
        self.v += 1
        return self.v < o.v


@fieldwise_init
struct H:
    var s: S


def main():
    var k = S(1)
    var holder = H(S(2))
    var l = S(30)
    print(holder.s < k < l)
