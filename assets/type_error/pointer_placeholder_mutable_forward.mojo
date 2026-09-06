# expect: type mismatch for argument
# Forwarding a bare placeholder-origin pointer parameter (`Pointer[T, _]`) to
# a parameter demanding mutable capability (`MutPointer[T, _]`) is rejected:
# the forwarding body cannot prove the capability upstream infers per call.
def w(q: MutPointer[Int, _]):
    q[unsafe_offset=0] = 3

def via(p: Pointer[Int, _]):
    w(p)

def main():
    var x = 1
    via(Pointer(to=x))
    print(x)
