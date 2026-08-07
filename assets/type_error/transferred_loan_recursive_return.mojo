# expect: returned reference escapes storage
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Sink:
    var slot: RefBox

    def ping(mut self, var box: RefBox, n: Int):
        if n > 0:
            self.pong(box^, n - 1)
        else:
            self.slot = box^

    def pong(mut self, var box: RefBox, n: Int):
        self.ping(box^, n)

def make(mut keep: List[Int]) -> Sink:
    ref whole = keep
    var sink = Sink(RefBox(whole))
    var local = [9]
    ref alias = local
    sink.pong(RefBox(alias), 1)
    return sink^

def main():
    var keep = [1]
    var got = make(keep)
