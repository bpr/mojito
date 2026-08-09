# Stored callables invoke through the field-invocation channel: a thin
# function field calls directly; a capturing closure field rehydrates its
# environment through the field's stable place, with the storage loaning
# its reference captures' owners.
@fieldwise_init
struct Holder:
    var callback: def(Int) -> Int

@fieldwise_init
struct Env:
    var callback: def(Int) capturing[_] -> Int

def double(x: Int) -> Int:
    return x * 2

def main():
    var holder = Holder(double)
    print(holder.callback(1))
    var n = 10
    def bump(x: Int) unified {imm n} -> Int:
        return x + n
    var env = Env(bump)
    print(env.callback(5))
    var fns: List[def(Int) -> Int] = [double]
    print((fns[0])(3))
