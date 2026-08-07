# expect: has no method
# A closure value can reach a callable-typed field through a constructor
# argument, but invocation through the field is not offered — so a stored
# closure that captured a loop reference can never be called after its
# referent died (closures remain downward funargs).
@fieldwise_init
struct Holder:
    var callback: def() -> Int

def main():
    var values = [1, 2]
    for ref x in values:
        def peek() unified {ref x} -> Int:
            return x
        var holder = Holder(peek)
        print(holder.callback())
