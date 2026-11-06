# expect: the SIMD type must be floating point
# `[1, 2, 2.5]` merges `1` and `2` at `Int`, which a run-time display
# materializes before merging `2.5`, which then converts to `Int` through
# upstream's `@implicit SIMD.__init__(FloatLiteral)` and fails its assert.
def main():
    var a = [1, 2, 2.5]
    print(len(a))
