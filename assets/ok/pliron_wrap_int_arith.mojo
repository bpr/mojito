# Defined two's-complement wrapping for Int + - * // % and unary negation
# (the shared native ABI contract, docs/native-abi.md), including the single
# overflowing signed division case MIN // -1 == MIN with MIN % -1 == 0.
def compute() -> Int:
    var max = 9223372036854775807
    var min = -max - 1
    var add_wrap = max + 1
    var mul_wrap = max * 2
    var sub_wrap = min - 1
    var div_wrap = min // -1
    var mod_wrap = min % -1
    var neg_wrap = -min
    return add_wrap + mul_wrap + sub_wrap + div_wrap + mod_wrap + neg_wrap

def main():
    print(compute())
