# expect: type mismatch for callable-value parameter
# Binding a CONSTRAINED function to a function type that declares no matching
# `where` clause is an error (upstream 2026-08): calls through the contract
# could otherwise violate the implementation's precondition.
def kernel[w: Int](x: Int) where (w > 0, "width must be positive"):
    print(w + x)

def apply[F: def[w: Int](Int) thin -> None](x: Int):
    F[4](x)

def main():
    apply[kernel](3)
