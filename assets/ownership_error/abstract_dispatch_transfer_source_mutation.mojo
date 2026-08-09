# expect: access to 'local' conflicts with live reference 'bag'
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

trait Sink:
    def put(mut self, var box: RefBox): ...

@fieldwise_init
struct Bag(Sink):
    var slot: RefBox

    def put(mut self, var box: RefBox):
        self.slot = box^

def feed[T: Sink](mut sink: T, var box: RefBox):
    sink.put(box^)

def feed(x: Int):
    print(x)

def main():
    var keep = [1]
    ref whole = keep
    var bag = Bag(RefBox(whole))
    var local = [9]
    ref alias = local
    feed(bag, RefBox(alias))
    local.append(1)
    print(bag.slot.value[0])
