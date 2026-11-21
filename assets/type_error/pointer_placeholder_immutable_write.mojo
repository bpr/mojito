# expect: expression must be mutable in assignment
# A bare placeholder-origin pointer parameter (`Pointer[T, _]`) infers its
# origin per call, so the body cannot prove the mutable capability a write
# through it needs: the store is rejected ("expression must be mutable in
# assignment"), as at the pin.
def bump(p: Pointer[Int, _]):
    p[unsafe_offset=0] = 2

def main():
    var x = 1
    bump(Pointer(to=x))
    print(x)
