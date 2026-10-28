# A function returning `!kgen.deferred` runs only at compile time: the pin
# cannot lower a run-time call of one.
# expect: failed to convert call result type: '_mlir_attr' returns '__mlir_type.`!kgen.deferred`', which only a compile-time position may call
from std.simd import FastMathFlag


def main():
    var a = FastMathFlag.FAST._mlir_attr()
    print("unreachable")
