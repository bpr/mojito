# expect: constraint failed: width must be positive
# An explicit specialization through a constrained callable parameter
# evaluates the contract's `where` clauses: `F[0]` violates `w > 0`.
def kernel[w: Int](x: Int) where (w > 0, "width must be positive"):
    print(w + x)

def apply[F: def[w: Int](Int) thin -> None where (w > 0, "width must be positive")](x: Int):
    F[0](x)

def main():
    apply[kernel](3)
