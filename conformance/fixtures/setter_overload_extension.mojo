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
    sink[2] = 7
    print(sink.value)
