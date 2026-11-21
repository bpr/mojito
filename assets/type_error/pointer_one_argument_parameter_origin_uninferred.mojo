# A parameter's `Pointer` type must bind its `origin` (or unbind it with `_`).
# expect: failed to infer parameter 'origin'
def read(p: Pointer[Int]) -> Int:
    return p[]

def main():
    var x = 5
    print(read(Pointer(to=x)))
