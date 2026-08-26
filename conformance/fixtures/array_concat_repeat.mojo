# mojo-only (strict-subset gap): upstream Array's consuming `concat`
# (`out result: Array[T, Self.length + rhs.length]`) and `repeat[n]`
# (`Array[T, Self.length * n]`) produce DEPENDENT result lengths — value-
# parameter arithmetic in a result type position, which Mojito's value-param
# type constructors do not support. Upstream prints `5 1 5` / `4 7 8`.
def main():
    var a: Array[Int, 2] = [1, 2]
    var b: Array[Int, 3] = [3, 4, 5]
    var c = a^.concat(b^)
    print(len(c), c[0], c[4])
    var d: Array[Int, 2] = [7, 8]
    var e = d^.repeat[2]()
    print(len(e), e[0], e[3])
