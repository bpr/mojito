# Current Mojo treats a bare `def(...)` struct-field type position as a trait,
# not a storable callable value; callable values are limited to parameters and
# local bindings.
# expect: type of struct field 'callback'
@fieldwise_init
struct Holder:
    var callback: def(Int) -> Int

def double(x: Int) -> Int:
    return x * 2

def main():
    var holder = Holder(double)
    print(holder.callback(21))
