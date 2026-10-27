# A subscript store passes its value by keyword, under the name of the first
# declared `__setitem__`'s last argument, typed at `__getitem__`'s result:
# here `value: Bool`, so the keyword-only overload is selected and the
# positional-only one is never viable.
@fieldwise_init
struct Sink:
    var value: Int

    def __getitem__(self, index: Int) -> Bool:
        return self.value != 0

    def __setitem__(mut self, index: Int, *, value: Bool):
        print("keyword")
        if value:
            self.value = index

    def __setitem__(mut self, index: Int, value: Int, /):
        print("positional")
        self.value = value

def main():
    var sink = Sink(0)
    sink[2] = True
    print(sink.value)
