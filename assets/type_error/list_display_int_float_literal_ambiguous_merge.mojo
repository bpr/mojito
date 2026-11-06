# expect: ambiguous merge: left value has type 'IntLiteral[1]' and right value has type 'FloatLiteral[2.5]'
# A list display merges its elements as a conditional expression merges
# its arms, so `[1, 2.5]` is the same ambiguous merge.
def main():
    var a = [1, 2.5]
    print(len(a))
