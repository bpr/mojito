# expect: access to 'n' conflicts with live reference 'env'
@fieldwise_init
struct Env:
    var callback: def(Int) capturing[_] -> Int

def main():
    var n = 10
    def bump(x: Int) unified {imm n} -> Int:
        return x + n
    var env = Env(bump)
    n += 1
    print(env.callback(5))
