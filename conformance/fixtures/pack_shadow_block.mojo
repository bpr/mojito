def inspect[*Ts: Movable & Deinitable](var *args: *Ts):
    if True:
        var args = Tuple(9, 10)
        var local = Tuple(*args^)
        print(local)


def main():
    inspect(1, True)
