@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Two:
    var a: List[RefBox]
    var b: List[Int]

def main():
    var a: List[RefBox] = List[RefBox]()
    var t = Two(a^, [1])
    var local = [9]
    ref alias = local
    t.a.append(RefBox(alias))
    var fresh: List[RefBox] = List[RefBox]()
    t.a = fresh^
    local.append(1)
    print(t.b[0], local[1])
