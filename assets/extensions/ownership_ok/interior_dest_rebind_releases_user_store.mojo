# Rebinding the exact interior destination releases its transferred-loan
# generation: after `t.a` is replaced the old alias is gone, so its source
# mutates freely while sibling storage stays live.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Carrier[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]

@fieldwise_init
struct Two[origin: Origin[mut=True]]:
    var a: Carrier[Self.origin]
    var b: List[Int]

def stash_into_a(mut t: Two, box: RefBox):
    t.a.slot = box^

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var t = Two(Carrier(RefBox(whole)), [1])
    var local: List[Int] = [9]
    ref alias = local
    stash_into_a(t, RefBox(alias))
    var keep2: List[Int] = [2]
    ref again = keep2
    t.a = Carrier(RefBox(again))
    local.append(1)
    print(t.b[0], local[1])
