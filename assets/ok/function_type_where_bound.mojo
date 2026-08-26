# A `thin` function type carries trailing `where` clauses constraining the
# parameters it declares (upstream 2026-08). The clauses lower onto the
# anonymous contract's parameter declarations, and binder names are
# alpha-renamed for identity: the implementation spells its width `n` while
# the contract spells it `w`.
def kernel[n: Int](x: Int) where (n > 0, "width must be positive"):
    print(n + x)

def apply[F: def[w: Int](Int) thin -> None where (w > 0, "width must be positive")](x: Int):
    F[4](x)

def main():
    apply[kernel](3)
