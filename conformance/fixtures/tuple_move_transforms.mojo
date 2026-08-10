@fieldwise_init
struct Token(Movable, Deinitable):
    var id: Int

def main():
    var pair = (Token(1), Token(2))
    var reversed = pair^.reverse()
    ref reversed_first = reversed[0]
    var reversed_first_id = reversed_first.id
    ref reversed_second = reversed[1]
    var reversed_second_id = reversed_second.id
    print(reversed_first_id, reversed_second_id)

    var left = Tuple(Token(3))
    var right = Tuple(Token(4))
    var joined = left^.concat(right^)
    ref joined_first = joined[0]
    var joined_first_id = joined_first.id
    ref joined_second = joined[1]
    var joined_second_id = joined_second.id
    print(joined_first_id, joined_second_id)
