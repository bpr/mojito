# expect: stored reference escapes storage
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Pair:
    var a: RefBox
    var b: Int

    def fill(mut self):
        var local = [9]
        ref alias = local
        var pack = (RefBox(alias), 5)
        self.a, self.b = pack^

def main():
    var keep = [1]
    ref whole = keep
    var pair = Pair(RefBox(whole), 0)
    pair.fill()
