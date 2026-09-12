# Rebinding the exact interior destination releases its transferred-loan
# generation: after `t.a` is replaced the old alias is gone, so its source
# mutates freely while sibling storage stays live.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

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
    var t = Two(Carrier(RefBox(Pointer(to=whole))), [1])
    var local: List[Int] = [9]
    ref view = local
    stash_into_a(t, RefBox(Pointer(to=view)))
    var keep2: List[Int] = [2]
    ref again = keep2
    t.a = Carrier(RefBox(Pointer(to=again)))
    local.append(1)
    print(t.b[0], local[1])
