# expect: requires exactly one zero-argument ref-returning overload
# A delegated-call origin expression must name a callee with an explicit
# ref-return contract; a value-returning method offers no origin to inherit.
@fieldwise_init
struct Box:
    var item: Int

    def get(self) -> Int:
        return self.item

@fieldwise_init
struct Wrap:
    var inner: Box

    def peek(self) -> ref[self.inner.get()] Int:
        return self.inner.get()

def main():
    pass
