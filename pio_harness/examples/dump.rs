fn main(){
 let a=pio::pio_asm!(".side_set 1 opt\nlow:\n.wrap_target\n wait 1 irq 0 side 1\n jmp PIN high\n.wrap\nhigh:\n wait 1 irq 0 side 0\n jmp low\n");
 let b=pio::pio_asm!(".side_set 1 opt\n.wrap_target\nactive_pull:\n pull\n mov x osr\n jmp x!=y bit_start\n irq set 0 [7]\n nop [4]\n nop side 0 [7]\n irq set 0\nidle_wait:\n pull side 0 [2]\n set x 2 side 1\npre_loop:\n jmp x-- pre_loop [7]\nbit_gap:\n nop [2]\nbit_start:\n irq set 0 side 1\n out x 1\n jmp x-- bit_one [2]\nbit_zero:\n jmp next_bit\nbit_one:\n irq set 0\nnext_bit:\n jmp !OSRE bit_gap\n.wrap\n");
 print!("tx_a:"); for o in a.program.code.iter(){print!(" {:04X}",o);} println!(" wrap={:?}",(a.program.wrap.source,a.program.wrap.target));
 print!("tx_b:"); for o in b.program.code.iter(){print!(" {:04X}",o);} println!(" wrap={:?}",(b.program.wrap.source,b.program.wrap.target));
}
