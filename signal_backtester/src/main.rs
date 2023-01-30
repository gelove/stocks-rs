use common::database::*;
use common::structs::*;
use strategies::*;
use strategies::supertrend::*;

fn get_candles_from_database(connection: &Database, symbol: &str, resolution: &str, start_timestamp: i64, end_timestamp: i64) -> Vec<Candle> {
  let candles_query = format!(
    "
    with most_recent_candle_snapshots as (
      select max(scraped_at) as scraped_at, symbol, resolution, timestamp from candles
      where scraped_at >= {start_timestamp} and scraped_at <= {end_timestamp} and symbol = '{symbol}' and resolution = '{resolution}'
      group by symbol, resolution, timestamp
    )
    select
      candles.symbol,
      candles.resolution,
      candles.timestamp,
      open,
      high,
      low,
      close,
      volume
    from most_recent_candle_snapshots
    join candles on most_recent_candle_snapshots.scraped_at = candles.scraped_at and 
      most_recent_candle_snapshots.timestamp = candles.timestamp and 
      most_recent_candle_snapshots.symbol = candles.symbol and 
      most_recent_candle_snapshots.resolution = candles.resolution
      where candles.timestamp >= {start_timestamp}
      and candles.timestamp <= {end_timestamp}
      and candles.resolution = '{resolution}'
      and candles.symbol = '{symbol}'
    ORDER BY candles.timestamp ASC
  "
  );
  // TODO: filter out current partial candle and only look at 100% closed candles?
  // TODO: how to check if candle_scraper process crashed and data is stale/partial?
  let candles = connection.get_rows_from_database::<Candle>(&candles_query);
  return candles;
}

fn get_quote_snapshots_from_database(connection: &Database, symbol: &str, start_timestamp: i64, end_timestamp: i64) -> Vec<QuoteSnapshot> {
  let quotes_query = format!(
    "
    select symbol, scraped_at, ask_price, bid_price, last_trade_price
    from quote_snapshots
    where symbol = '{symbol}' and scraped_at >= {start_timestamp} and scraped_at <= {end_timestamp}
    order by scraped_at desc
    limit 1
    "
  );
  let quote_snapshots = connection.get_rows_from_database::<QuoteSnapshot>(&quotes_query);
  return quote_snapshots;
}

fn main() {
  // logger
  simple_logger::init_with_level(log::Level::Info).unwrap();
  // runtime
  let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
  // run
  rt.block_on(async {
    // config
    let symbol = "SPY";
    let resolution = "1";
    let indicator_settings = SupertrendStrategyIndicatorSettings {
      supertrend_periods: 10,
      supertrend_multiplier: 3.00,
    };
    let warmed_up_index = 0; // TODO: make this at least 1?
    // open database
    let connection = Database::new("./database.db");
    // init database tables
    connection.migrate("./schema/");
    // time
    let date = "2023-01-30 00:00:00";
    let (regular_market_start, regular_market_end) = common::market_session::get_regular_market_session_start_and_end_from_string(date);
    let regular_market_start_timestamp = regular_market_start.timestamp();
    let mut pointer = regular_market_start;
    // state
    let mut trade_direction = Direction::Flat;
    while pointer <= regular_market_end {
      let eastern_now = &pointer;
      let eastern_now_timestamp = eastern_now.timestamp();
      let (current_candle_start, current_candle_end) = common::market_session::get_current_candle_start_and_stop(resolution, &eastern_now);
      let current_candle_start_timestamp = current_candle_start.timestamp();
      let current_candle_end_timestamp = current_candle_end.timestamp();
      // TODO: which is better, follow current timestamp with no delay or always look to previous closed candle
      //let candle_lookup_max_timestamp = eastern_now_timestamp;
      //let expected_max_signal_snapshot_age = 60;
      let candle_lookup_max_timestamp = current_candle_start_timestamp - 1;
      let expected_max_signal_snapshot_age = 120;
      let candles = get_candles_from_database(&connection, symbol, resolution, regular_market_start_timestamp, candle_lookup_max_timestamp);
      // get most recent signal signal from candles
      let strategy = SupertrendStrategy::new();
      let signal_snapshots = strategy.build_signal_snapshots_from_candles(&indicator_settings, &candles);
      if signal_snapshots.is_empty() {
        log::warn!("{eastern_now_timestamp}: signal_snapshots.len() == 0");
        pointer = pointer + chrono::Duration::seconds(1);
        continue;
      }
      // get direction changes from signal snapshots
      let direction_changes = build_direction_changes_from_signal_snapshots(&signal_snapshots, warmed_up_index);
      if direction_changes.is_empty() {
        log::warn!("{eastern_now_timestamp}: direction_changes.len() == 0");
        pointer = pointer + chrono::Duration::seconds(1);
        continue;
      }
      let most_recent_direction_change = &direction_changes[direction_changes.len() - 1];
      // get current quote
      let quote_snapshots = get_quote_snapshots_from_database(&connection, symbol, regular_market_start_timestamp, eastern_now_timestamp);
      if quote_snapshots.is_empty() {
        log::warn!("{eastern_now_timestamp}: quote_snapshots.len() == 0");
        pointer = pointer + chrono::Duration::seconds(1);
        continue;
      }
      let most_recent_quote_snapshot = &quote_snapshots[0];
      // check quote age
      let quote_age = eastern_now_timestamp - most_recent_quote_snapshot.scraped_at;
      // TODO: handle if quote_snapshot is too old/unrealistic from something like a quote_scraper process crash
      if quote_age > 1 {
        log::warn!("{eastern_now_timestamp}: quote_snapshot is old! quote_age = {quote_age}");
      }
      // check snapshot age?
      let most_recent_signal_snapshot = &signal_snapshots[signal_snapshots.len() - 1];
      let signal_snapshot_age = eastern_now_timestamp - most_recent_signal_snapshot.candle.timestamp;
      if signal_snapshot_age > expected_max_signal_snapshot_age {
        log::warn!("{eastern_now_timestamp}: signal_snapshot is old! signal_snapshot_age = {signal_snapshot_age}");
      }
      // log
      log::info!(
        "{eastern_now_timestamp}: current_candle = {current_candle_start_timestamp}-{current_candle_end_timestamp} quote_age = {quote_age}s snapshot_age = {signal_snapshot_age}s direction = {:?} signal_snapshot = {:?} quote_snapshot = {:?}",
        most_recent_direction_change,
        most_recent_signal_snapshot,
        most_recent_quote_snapshot
      );
      // increment pointer
      pointer = pointer + chrono::Duration::seconds(1);
    }
  });
}
