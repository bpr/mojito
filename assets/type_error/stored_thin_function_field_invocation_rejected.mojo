# expect: has no method
# A thin (capture-free) function stores legally into a callable field, but
# invocation through the field is still not offered — stored callables stay
# inert pending the field-invocation channel.
@fieldwise_init
struct Holder:
    var callback: def(Int) -> Int

def double(x: Int) -> Int:
    return x * 2

def main():
    var holder = Holder(double)
    print(holder.callback(1))
