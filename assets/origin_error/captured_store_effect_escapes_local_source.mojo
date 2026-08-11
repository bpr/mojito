# expect: escapes storage
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Keeper:
    var slot: RefBox

    def add_local(mut self):
        var local: List[Int] = [9]
        ref alias = local
        def push(var box: RefBox) {mut self}:
            self.slot = box^
        push(RefBox(alias))

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var k = Keeper(RefBox(whole))
    k.add_local()
