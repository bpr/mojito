# expect: doesn't have a memory origin
# A `ref` binding needs a memory origin: a register-passable temporary
# subject has none.
def main():
    match Int():
        case ref z:
            print(z)
