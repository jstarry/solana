import React from "react";
import { PublicKey, Connection, StakeActivationData } from "@solana/web3.js";
import { useCluster, Cluster } from "../cluster";
import { HistoryProvider } from "./history";
import { TokensProvider } from "./tokens";
import { create } from "superstruct";
import { ParsedInfo } from "validators";
import { StakeAccount } from "validators/accounts/stake";
import * as Cache from "providers/cache";
import { ActionType, FetchStatus } from "providers/cache";
import { reportError } from "utils/sentry";
import { VoteAccount } from "validators/accounts/vote";
import { NonceAccount } from "validators/accounts/nonce";
import { SysvarAccount } from "validators/accounts/sysvar";
import { ConfigAccount } from "validators/accounts/config";
import { FlaggedAccountsProvider } from "./flagged-accounts";
import {
  ProgramDataAccount,
  ProgramDataAccountInfo,
  UpgradeableLoaderAccount,
} from "validators/accounts/upgradeable-program";
import { RewardsProvider } from "./rewards";
import { NAME_PROGRAM_ID } from "@solana/spl-name-service";
export { useAccountHistory } from "./history";

type Names = string[];
type State = Cache.State<Names>;
type Dispatch = Cache.Dispatch<Names>;

const StateContext = React.createContext<State | undefined>(undefined);
const DispatchContext = React.createContext<Dispatch | undefined>(undefined);

type ProviderProps = { children: React.ReactNode };
export function NamesProvider({ children }: ProviderProps) {
  const { url } = useCluster();
  const [state, dispatch] = Cache.useReducer<Names>(url);

  return (
    <StateContext.Provider value={state}>
      <DispatchContext.Provider value={dispatch}>
        {children}
      </DispatchContext.Provider>
    </StateContext.Provider>
  );
}

export function useNames(
  address: string | undefined
): Cache.CacheEntry<Names> | undefined {
  const context = React.useContext(StateContext);

  if (!context) {
    throw new Error(`useNames must be used within a AccountsProvider`);
  }
  if (address === undefined) return;
  return context.entries[address];
}

async function fetchNames(
  dispatch: Dispatch,
  pubkey: PublicKey,
  cluster: Cluster,
  url: string
) {
  dispatch({
    type: ActionType.Update,
    key: pubkey.toBase58(),
    status: Cache.FetchStatus.Fetching,
    url,
  });

  let data;
  let fetchStatus;
  try {
    const connection = new Connection(url, "confirmed");
    connection.getProgramAccounts(NAME_PROGRAM_ID, {
        commitment: "confirmed",
        dataSlice: {offset: 0, length: 0},
        filters: [{
            memcmp: {
                offset: 0,
                bytes: "58PwtjSDuFHuUkYjH9BYnnQKHfwo9reZhC2zMJv9JPkx",
            },
        },
        {
            memcmp: {
                offset: 32,
                bytes: pubkey.toBase58(),
            }
        }],
    });
    const result = (await connection.getParsedAccountInfo(pubkey)).value;

    let lamports, details;
    if (result === null) {
      lamports = 0;
    } else {
      lamports = result.lamports;

      // Only save data in memory if we can decode it
      let space: number;
      if (!("parsed" in result.data)) {
        space = result.data.length;
      } else {
        space = result.data.space;
      }

      let data: ProgramData | undefined;
      if ("parsed" in result.data) {
        try {
          const info = create(result.data.parsed, ParsedInfo);
          switch (result.data.program) {
            case "bpf-upgradeable-loader": {
              const parsed = create(info, UpgradeableLoaderAccount);

              // Fetch program data to get program upgradeability info
              let programData: ProgramDataAccountInfo | undefined;
              if (parsed.type === "program") {
                const result = (
                  await connection.getParsedAccountInfo(parsed.info.programData)
                ).value;
                if (
                  result &&
                  "parsed" in result.data &&
                  result.data.program === "bpf-upgradeable-loader"
                ) {
                  const info = create(result.data.parsed, ParsedInfo);
                  programData = create(info, ProgramDataAccount).info;
                } else {
                  throw new Error(
                    `invalid program data account for program: ${pubkey.toBase58()}`
                  );
                }
              }

              data = {
                program: result.data.program,
                parsed,
                programData,
              };

              break;
            }
            case "stake": {
              const parsed = create(info, StakeAccount);
              const isDelegated = parsed.type === "delegated";
              const activation = isDelegated
                ? await connection.getStakeActivation(pubkey)
                : undefined;

              data = {
                program: result.data.program,
                parsed,
                activation,
              };
              break;
            }
            case "vote":
              data = {
                program: result.data.program,
                parsed: create(info, VoteAccount),
              };
              break;
            case "nonce":
              data = {
                program: result.data.program,
                parsed: create(info, NonceAccount),
              };
              break;
            case "sysvar":
              data = {
                program: result.data.program,
                parsed: create(info, SysvarAccount),
              };
              break;
            case "config":
              data = {
                program: result.data.program,
                parsed: create(info, ConfigAccount),
              };
              break;

            case "spl-token":
              data = {
                program: result.data.program,
                parsed: create(info, TokenAccount),
              };
              break;
            default:
              data = undefined;
          }
        } catch (error) {
          reportError(error, { url, address: pubkey.toBase58() });
        }
      }

      details = {
        space,
        executable: result.executable,
        owner: result.owner,
        data,
      };
    }
    data = { pubkey, lamports, details };
    fetchStatus = FetchStatus.Fetched;
  } catch (error) {
    if (cluster !== Cluster.Custom) {
      reportError(error, { url });
    }
    fetchStatus = FetchStatus.FetchFailed;
  }
  dispatch({
    type: ActionType.Update,
    status: fetchStatus,
    data,
    key: pubkey.toBase58(),
    url,
  });
}

export function useFetchNames() {
  const dispatch = React.useContext(DispatchContext);
  if (!dispatch) {
    throw new Error(
      `useFetchNames must be used within a AccountsProvider`
    );
  }

  const { cluster, url } = useCluster();
  return React.useCallback(
    (pubkey: PublicKey) => {
      fetchNames(dispatch, pubkey, cluster, url);
    },
    [dispatch, cluster, url]
  );
}
