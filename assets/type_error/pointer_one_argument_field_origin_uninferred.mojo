# A field's `Pointer` type must bind its `origin`, which has no default.
# expect: failed to infer parameter 'origin'
struct Holder:
    var data: Pointer[Int]

def main():
    print(1)
