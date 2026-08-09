# Current Mojo rejects a competing `__setitem__` pair whose assignment value
# is positional in one overload and keyword-only in the other over the same
# index types.
# expect: competing '__setitem__'
@fieldwise_init
struct Sink:
    var value: Int

    def __setitem__(mut self, index: Int, value: Int, /):
        self.value = value

    def __setitem__(mut self, index: Int, *, value: Bool):
        if value:
            self.value = index

def main():
    var sink = Sink(0)
    sink[1] = True
    print(sink.value)
