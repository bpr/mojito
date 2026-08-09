# expect: access to 'local' conflicts with live reference 't'
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Carrier:
    var slot: RefBox

@fieldwise_init
struct Two:
    var a: Carrier
    var b: List[Int]

def stash_into_a(mut t: Two, box: RefBox):
    t.a.slot = box^

def main():
    var keep = [1]
    ref whole = keep
    var t = Two(Carrier(RefBox(whole)), [1])
    var local = [9]
    ref alias = local
    stash_into_a(t, RefBox(alias))
    local.append(1)
    print(t.b[0])
