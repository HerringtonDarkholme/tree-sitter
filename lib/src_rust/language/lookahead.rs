use core::{ffi::c_void, ptr};

use crate::ffi::{TSLanguage, TSStateId, TSSymbol};

use super::super::alloc::{free, malloc};
use super::super::utils::ptr_mut;
use super::{lang, parse_action_at, parse_action_entry, ts_language_symbol_name, TSParseAction};

/// Iterator over valid lookahead symbols for a given parse state.
///
/// The public API treats this as an opaque heap allocation, so its fields use
/// Rust layout. Only the current result is visible to other runtime modules.
pub struct LookaheadIterator {
    /// Language whose tables are being scanned.
    language: *const TSLanguage,
    /// Current raw table cursor.
    data: *const u16,
    /// End of the current small-state symbol group.
    group_end: *const u16,
    /// Parse-table value for the current symbol.
    table_value: u16,
    /// Remaining grouped-symbol count in the current section.
    group_count: u16,
    /// Whether this iterator is scanning small-state data.
    is_small_state: bool,

    /// Current symbol's action list.
    pub(in super::super) actions: *const TSParseAction,
    /// Current lookahead symbol.
    pub(in super::super) symbol: TSSymbol,
    /// Shift/goto state for current symbol when applicable.
    pub(in super::super) next_state: TSStateId,
    /// Number of current actions.
    pub(in super::super) action_count: u16,
}

/// Create a lookahead iterator for a given state.
#[inline]
pub unsafe fn language_lookaheads(self_: *const TSLanguage, state: TSStateId) -> LookaheadIterator {
    let l = lang(self_);
    let is_small_state = u32::from(state) >= l.large_state_count;
    let (data, group_end, group_count): (*const u16, *const u16, u16) = if is_small_state {
        let index = *l
            .small_parse_table_map
            .add(state as usize - l.large_state_count as usize);
        let data = l.small_parse_table.add(index as usize);
        (data, data.add(1), *data)
    } else {
        (
            l.parse_table
                .add(state as usize * l.symbol_count as usize)
                .sub(1),
            ptr::null(),
            0,
        )
    };
    LookaheadIterator {
        language: self_,
        data,
        group_end,
        table_value: 0,
        group_count,
        is_small_state,
        actions: ptr::null(),
        symbol: u16::MAX,
        next_state: 0,
        action_count: 0,
    }
}

/// Advance a lookahead iterator to the next valid symbol.
#[inline]
pub unsafe fn lookahead_iterator_next(self_: &mut LookaheadIterator) -> bool {
    let l = lang(self_.language);

    if self_.is_small_state {
        self_.data = self_.data.add(1);
        if self_.data == self_.group_end {
            if self_.group_count == 0 {
                return false;
            }
            self_.group_count -= 1;
            self_.table_value = *self_.data;
            self_.data = self_.data.add(1);
            let symbol_count = *self_.data;
            self_.data = self_.data.add(1);
            self_.group_end = self_.data.add(symbol_count as usize);
            self_.symbol = *self_.data;
        } else {
            self_.symbol = *self_.data;
            return true;
        }
    } else {
        loop {
            self_.data = self_.data.add(1);
            self_.symbol = self_.symbol.wrapping_add(1);
            if self_.symbol >= l.symbol_count as u16 {
                return false;
            }
            self_.table_value = *self_.data;
            if self_.table_value != 0 {
                break;
            }
        }
    }

    // Depending on if the symbol is terminal or non-terminal, the table value
    // either represents a list of actions or a successor state.
    let language = l;
    if u32::from(self_.symbol) < language.token_count {
        let entry = parse_action_entry(language, self_.table_value as usize);
        self_.action_count = u16::from(entry.entry.count);
        self_.actions = parse_action_at(language, self_.table_value as usize + 1);
        self_.next_state = 0;
    } else {
        self_.action_count = 0;
        self_.next_state = self_.table_value;
    }
    true
}

// ---------------------------------------------------------------------------
// Lookahead iterator public API
// ---------------------------------------------------------------------------

/// `TSLookaheadIterator` is an opaque handle = `LookaheadIterator` allocated on heap.
#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_new(
    self_: *const TSLanguage,
    state: TSStateId,
) -> *mut LookaheadIterator {
    if u32::from(state) >= lang(self_).state_count {
        return ptr::null_mut();
    }
    let iterator = malloc(core::mem::size_of::<LookaheadIterator>()).cast::<LookaheadIterator>();
    ptr::write(iterator, language_lookaheads(self_, state));
    iterator
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_delete(self_: *mut LookaheadIterator) {
    free(self_.cast::<c_void>());
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_reset_state(
    self_: *mut LookaheadIterator,
    state: TSStateId,
) -> bool {
    if u32::from(state) >= lang((*self_).language).state_count {
        return false;
    }
    *self_ = language_lookaheads((*self_).language, state);
    true
}

#[no_mangle]
pub const unsafe extern "C" fn ts_lookahead_iterator_language(
    self_: *const LookaheadIterator,
) -> *const TSLanguage {
    (*self_).language
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_reset(
    self_: *mut LookaheadIterator,
    language: *const TSLanguage,
    state: TSStateId,
) -> bool {
    if u32::from(state) >= lang(language).state_count {
        return false;
    }
    *self_ = language_lookaheads(language, state);
    true
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_next(self_: *mut LookaheadIterator) -> bool {
    lookahead_iterator_next(ptr_mut(self_))
}

#[no_mangle]
pub const unsafe extern "C" fn ts_lookahead_iterator_current_symbol(
    self_: *const LookaheadIterator,
) -> TSSymbol {
    (*self_).symbol
}

#[no_mangle]
pub unsafe extern "C" fn ts_lookahead_iterator_current_symbol_name(
    self_: *const LookaheadIterator,
) -> *const i8 {
    ts_language_symbol_name((*self_).language, (*self_).symbol)
}
