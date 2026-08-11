# Array with struct elements: the display moves its temporaries in, owned
# iteration consumes the array, and the keyword `fill` constructor copies its
# argument into every slot.
@fieldwise_init
struct Box:
    var v: Int

def main():
    var bs: Array[Box, 2] = [Box(3), Box(4)]
    print(bs[0].v + bs[1].v)
    var t = 0
    for var b in bs^:
        t += b.v
    print(t)
    var f = Array[Int, 4](fill=9)
    print(f[3], len(f))
