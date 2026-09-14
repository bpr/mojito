# expect: write through a Pointer whose origin is immutable
# The empty-subscript store through a symbolic-origin pointer field is judged
# by the same binding.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Int, Self.o]

def show(x: Int):
    var p = P(Pointer(to=x))
    p.src[] = 3

def main():
    var x = 7
    show(x)
