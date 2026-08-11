# expect: access to 'local' conflicts with live reference 'sink'
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Carrier:
    var slot: RefBox

def stash(mut sink: Carrier, box: RefBox):
    sink.slot = box^

def stash(x: Int):
    print(x)

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var sink = Carrier(RefBox(whole))
    var local: List[Int] = [9]
    ref alias = local
    stash(sink, RefBox(alias))
    local.append(1)
    print(sink.slot.value[0])
