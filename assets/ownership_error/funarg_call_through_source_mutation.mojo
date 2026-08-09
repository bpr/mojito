# expect: access to 'local' conflicts with live reference 'sink'
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Carrier:
    var slot: RefBox

def stash(mut sink: Carrier, box: RefBox):
    sink.slot = box^

def feed[callback: def(mut Carrier, RefBox) thin](mut sink: Carrier, box: RefBox):
    callback(sink, box^)

def main():
    var keep = [1]
    ref whole = keep
    var sink = Carrier(RefBox(whole))
    var local = [9]
    ref alias = local
    feed[stash](sink, RefBox(alias))
    local.append(1)
    print(sink.slot.value[0])
