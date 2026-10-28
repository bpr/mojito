# `pop.select`'s two cases and its result are of one type; the pin's
# verifier rejects an `Int` and a `UInt8`.
# expect: invalid call to 'pop.select': op failed to verify that all of {trueValue, falseValue, result} have same type
def main():
    var c = 3 > 2
    print(__mlir_op.`pop.select`(c.__mlir_bool__(), Int(1), UInt8(2)))
