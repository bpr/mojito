# Mojito accepts `read` as an argument name; the pin rejects it: "'read' was
# removed; use 'imm'".
def same(read: Int) -> Int:
    return read + 1


def main():
    print(same(1))
