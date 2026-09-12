# A value-parameterized generic `def` folds at compile time: `choose_width[4]()`
# is CTFE-evaluated into the module-level `comptime W`, and a second alias folds
# the comparison, so the printed `True` pins the folded value. (The assertion is
# an alias rather than a module-level `comptime if`, which upstream requires
# inside a function.)
def choose_width[n: Int]() -> Int:
    if n < 8:
        return 8
    return n

comptime W = choose_width[4]()
comptime W_IS_8 = W == 8

def main():
    print(W)
    print(W_IS_8)
