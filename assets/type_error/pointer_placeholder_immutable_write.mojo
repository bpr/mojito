# expect: immutable origin
# A bare placeholder-origin pointer parameter (`Pointer[T, _]`) infers its
# origin per call, so the body cannot prove the mutable capability a write
# through it needs: upstream rejects the store ("expression must be mutable
# in assignment"), and so does Mojito's immutable reading of the spelling.
def bump(p: Pointer[Int, _]):
    p[unsafe_offset=0] = 2

def main():
    var x = 1
    bump(Pointer(to=x))
    print(x)
