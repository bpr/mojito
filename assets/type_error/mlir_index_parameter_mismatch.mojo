# Two `index` arguments that differ name two types, as at the pin.
# expect: type mismatch for variable 'x': expected S[3], found S[4]
@fieldwise_init
struct S[n: __mlir_type.index](Copyable, Movable):
    pass


def main():
    var x: S[Int(3).__mlir_index__()] = S[Int(4).__mlir_index__()]()
    _ = x^
