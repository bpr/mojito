def repack[*Ts: Movable](var *args: *Ts) -> Tuple[*Ts]:
    return Tuple(*args^)


def main():
    var values = repack()
    print(len(values))
