# A stored closure's reference captures loan their owners only while the
# storage lives: after its last use the owner mutates freely.
@fieldwise_init
struct Env:
    var callback: def(Int) capturing[_] -> Int

def main():
    var n = 10
    def bump(x: Int) unified {imm n} -> Int:
        return x + n
    var env = Env(bump)
    print(env.callback(5))
    n += 1
    print(n)
