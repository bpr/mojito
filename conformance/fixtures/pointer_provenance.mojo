from std.memory import unsafe_alloc

def main():
    var base = unsafe_alloc[Int](4, alignment=16)
    base[0] = 10
    base[1] = 20
    var next = base + 1
    print(next[0], next - base, next == base + 1)
    base.free()
