# expect: ambiguous merge: left value has type 'IntLiteral[1]' and right value has type 'FloatLiteral[2.5]'
# The arms of `1 if c else 2.5` each convert to the other's
# materialization target (`Int` and `Float64`), so upstream's
# `findCommonType` reports the merge ambiguous.
def main():
    var c = True
    var x = 1 if c else 2.5
    print(x)
