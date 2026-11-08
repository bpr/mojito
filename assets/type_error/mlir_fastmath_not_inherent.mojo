# expect: attribute 'fastmathFlags' is not an inherent attribute of 'pop.div'
# Only `pop.add`, `pop.sub`, `pop.mul`, `pop.fma`, and `pop.cast` hold a
# `fastmathFlags` property; the pin rejects it on every other op.
def main():
    var a = Float32(1.5)
    var r = __mlir_op.`pop.div`[fastmathFlags=__mlir_attr.`#pop.fmf<fast>`](a._mlir_value, a._mlir_value)
    print(Float32(mlir_value=r))
