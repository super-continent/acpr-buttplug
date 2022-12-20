use std::arch::asm;

use crate::helpers::Offset;
use detour::RawDetour;
use once_cell::sync::OnceCell;

const HANDLE_HIT_OFFSET: Offset = Offset::new(0x11AA80);

static HIT_OFFSET_DETOUR: OnceCell<RawDetour> = OnceCell::new();

pub unsafe fn setup_hooks() {
    log::trace!("setting up hooks...");
    let handle_hit_addr = HANDLE_HIT_OFFSET.get_address();

    log::debug!("got handle_hit offset: {:X}", handle_hit_addr);

    let detour = HIT_OFFSET_DETOUR.get_or_init(|| {
        RawDetour::new(handle_hit_addr as *const (), handle_hit_hook as *const ())
            .expect("initializing handle_hit detour")
    });

    if let Err(e) = detour.enable() {
        log::debug!("error: {e}")
    } else {
        log::info!("enabled handle_hit hook");
    }
}

#[naked]
unsafe extern "C" fn handle_hit_hook() {
    asm!(
        "
        push ebp
        mov ebp, esp
        push edx
        push ecx
        push eax
        mov ecx, eax
        mov edx, [ebp + 0xC]
        push edx
        mov edx, [ebp + 0x8]
        push edx
        call {0}
        pop eax
        pop ecx
        pop edx
        pop ebp
        ret",
        sym hit_hook,
        options(noreturn)
    )
}

#[no_mangle]
unsafe extern "thiscall" fn hit_hook(this: usize, arg2: usize, arg3: usize) {
    use crate::dll_code::Event;
    log::trace!("called hit_hook with arg: {:X?}", arg2);

    if let Some(channel) = crate::dll_code::HIT_CHANNEL_TX.blocking_lock().as_mut() {
        channel.send(Event::Hit).expect("Sending Event across channel");
    }

    let trampoline = HIT_OFFSET_DETOUR.get().unwrap().trampoline() as *const _;

    // push args to stack and move arg1 into eax,
    // then clear arguments off stack
    asm!(
        "push {0}
        push {1}
        mov eax, {2}
        call {3}
        add esp, 0x8",
        in(reg) arg3,
        in(reg) arg2,
        in(reg) this,
        in(reg) trampoline
    );
}
