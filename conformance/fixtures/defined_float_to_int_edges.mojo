# `Int(f)` is the saturating conversion in Mojito (`docs/native-abi.md`), and
# a NaN converts to zero. Upstream's is poison out of range, so the pin folds
# these branches to garbage.
def main():
    var big = 10000000000000000000.0
    var neg_big = -10000000000000000000.0
    var zero = 0.0
    print(Int(big), Int(neg_big), Int(zero / zero))
