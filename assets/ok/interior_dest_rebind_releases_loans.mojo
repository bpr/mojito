@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

@fieldwise_init
struct Two[origin: Origin[mut=True]]:
    var a: List[RefBox[Self.origin]]
    var b: List[Int]

def main():
    var local: List[Int] = [9]
    ref view = local
    var a = List[RefBox[origin_of(view)]]()
    var t = Two(a^, [1])
    t.a.append(RefBox(Pointer(to=view)))
    var fresh = List[RefBox[origin_of(view)]]()
    t.a = fresh^
    local.append(1)
    print(t.b[0], local[1])
