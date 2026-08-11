# A lambda materializes its closure at the expression's evaluation point: a
# `{var}` capture inside a loop snapshots once per iteration, and a lambda in
# a ternary branch is created only when that branch is taken.
def pick(flag: Bool) -> Int:
    return (lambda -> Int: 1)() if flag else (lambda -> Int: 2)()

def main():
    var i = 0
    var total = 0
    while i < 3:
        total = total + (lambda {var i} -> Int: i)()
        i = i + 1
    print(total)
    print(pick(True), pick(False))
