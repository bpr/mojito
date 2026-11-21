# An expression `Pointer[Int]` is the partial application `Pointer[Int, _]`,
# whose parent-struct parameters a static call cannot infer.
# expect: failed to infer parameter 'mut'
def main():
    var p = Pointer[Int].unsafe_dangling()
    print(1)
